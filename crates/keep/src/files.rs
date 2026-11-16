// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! A certificate, its key and the CA bundle as three files in a directory:
//! `cert.pem`, `key.pem` (mode 0600) and `ca.pem`. The agent keeps its
//! client certificate this way and a server its listener certificate.
//!
//! Replacing the set is done in two steps so that a crash never leaves a
//! certificate beside the wrong key. [`stage`] writes the new set to
//! `next/` and changes nothing else. [`commit`] writes `next/ready` and
//! then moves the files into place one rename at a time. [`recover`], run
//! at start, finishes a commit that was cut short and throws away a set
//! that was staged but never committed.

use std::io;
use std::path::Path;

use crate::tls::Pki;

const FILES: [&str; 3] = ["key.pem", "cert.pem", "ca.pem"];
const NEXT: &str = "next";
const READY: &str = "ready";

/// The set in `dir`, if all three files are there and not empty.
pub fn load(dir: &Path) -> io::Result<Option<Pki>> {
    let cert = dir.join("cert.pem");
    let key = dir.join("key.pem");
    let ca = dir.join("ca.pem");
    if !(non_empty(&cert) && non_empty(&key) && non_empty(&ca)) {
        return Ok(None);
    }
    Ok(Some(Pki {
        cert_pem: std::fs::read_to_string(&cert)?,
        key_pem: std::fs::read_to_string(&key)?,
        ca_pem: std::fs::read_to_string(&ca)?,
    }))
}

/// Write the set straight into `dir`. For a first certificate; to replace
/// one, [`stage`] and [`commit`].
pub fn save(dir: &Path, pki: &Pki) -> io::Result<()> {
    std::fs::create_dir_all(dir)?;
    std::fs::write(dir.join("cert.pem"), &pki.cert_pem)?;
    write_key(&dir.join("key.pem"), &pki.key_pem)?;
    std::fs::write(dir.join("ca.pem"), &pki.ca_pem)
}

fn write_key(path: &Path, pem: &str) -> io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(pem.as_bytes())?;
    // The mode above only applies to a file this call created.
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
}

fn non_empty(path: &Path) -> bool {
    std::fs::metadata(path)
        .map(|m| m.is_file() && m.len() > 0)
        .unwrap_or(false)
}

/// Write `pki` to `dir/next`, replacing anything staged before. The set
/// in `dir` is not touched.
pub fn stage(dir: &Path, pki: &Pki) -> io::Result<()> {
    let next = dir.join(NEXT);
    if next.exists() {
        std::fs::remove_dir_all(&next)?;
    }
    save(&next, pki)
}

/// Make the staged set the one in `dir`.
pub fn commit(dir: &Path) -> io::Result<()> {
    std::fs::write(dir.join(NEXT).join(READY), b"")?;
    promote(dir)
}

/// Finish or drop whatever is in `dir/next`. True when a commit that had
/// been cut short was finished.
pub fn recover(dir: &Path) -> io::Result<bool> {
    let next = dir.join(NEXT);
    if !next.is_dir() {
        return Ok(false);
    }
    if next.join(READY).exists() {
        promote(dir)?;
        return Ok(true);
    }
    std::fs::remove_dir_all(&next)?;
    Ok(false)
}

/// Each file moves with one rename and `ready` goes last, with the
/// directory, so this can run again after a crash at any point.
fn promote(dir: &Path) -> io::Result<()> {
    let next = dir.join(NEXT);
    for f in FILES {
        let from = next.join(f);
        if from.exists() {
            std::fs::rename(&from, dir.join(f))?;
        }
    }
    std::fs::remove_dir_all(&next)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pki(tag: &str) -> Pki {
        Pki {
            cert_pem: format!("cert-{tag}"),
            key_pem: format!("key-{tag}"),
            ca_pem: format!("ca-{tag}"),
        }
    }

    fn on_disk(dir: &Path) -> (String, String, String) {
        let p = load(dir).unwrap().unwrap();
        (p.cert_pem, p.key_pem, p.ca_pem)
    }

    fn set(tag: &str) -> (String, String, String) {
        let p = pki(tag);
        (p.cert_pem, p.key_pem, p.ca_pem)
    }

    #[test]
    fn a_staged_set_changes_nothing_until_it_is_committed() {
        let dir = tempfile::tempdir().unwrap();
        assert!(load(dir.path()).unwrap().is_none());
        save(dir.path(), &pki("old")).unwrap();
        stage(dir.path(), &pki("new")).unwrap();
        assert_eq!(on_disk(dir.path()), set("old"));
        commit(dir.path()).unwrap();
        assert_eq!(on_disk(dir.path()), set("new"));
        assert!(!dir.path().join(NEXT).exists());
    }

    #[test]
    fn a_set_staged_but_never_committed_is_dropped_at_start() {
        let dir = tempfile::tempdir().unwrap();
        save(dir.path(), &pki("old")).unwrap();
        stage(dir.path(), &pki("new")).unwrap();
        assert!(!recover(dir.path()).unwrap());
        assert_eq!(on_disk(dir.path()), set("old"));
        assert!(!dir.path().join(NEXT).exists());
        // Nothing staged: nothing to do.
        assert!(!recover(dir.path()).unwrap());
    }

    #[test]
    fn a_commit_cut_short_is_finished_from_any_point() {
        // The move stopped after 0, 1, 2 or all 3 of the files.
        for moved in 0..=FILES.len() {
            let dir = tempfile::tempdir().unwrap();
            save(dir.path(), &pki("old")).unwrap();
            stage(dir.path(), &pki("new")).unwrap();
            let next = dir.path().join(NEXT);
            std::fs::write(next.join(READY), b"").unwrap();
            for f in &FILES[..moved] {
                std::fs::rename(next.join(f), dir.path().join(f)).unwrap();
            }
            assert!(recover(dir.path()).unwrap());
            assert_eq!(on_disk(dir.path()), set("new"), "after {moved} files");
            assert!(!next.exists());
        }
    }

    #[test]
    fn the_key_file_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("key.pem"), "loose").unwrap();
        save(dir.path(), &pki("a")).unwrap();
        stage(dir.path(), &pki("b")).unwrap();
        commit(dir.path()).unwrap();
        let mode = std::fs::metadata(dir.path().join("key.pem"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}
