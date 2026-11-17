// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The share directory's owner and mode: what makes an exported share
//! writable by the clients the export line allows.
//!
//! Every export line squashes every client to one uid and gid (see
//! [`crate::export_opts`]), so the directory must be owned by that pair and
//! writable by it. The convention for a simple managed share is
//! nobody/nogroup, `65534` on Linux. `PISTACK_SHELF_SQUASH_UID` overrides the
//! pair when a machine names `nobody` differently, and tests inject their own
//! uid because chowning to `65534` needs root.
//!
//! The mode is `0770` plus setgid. Setgid gives every file and directory made
//! inside the share the share's group, so the squash stays complete one level
//! down: a file one client wrote is still writable by the squashed uid of the
//! next client.

use std::path::Path;

/// nobody/nogroup on Linux: the uid and gid an export line squashes every
/// client to, and the owner a share directory carries. One number for both
/// names on every machine that has either.
pub const SQUASH_UID: u32 = 65534;

/// `rwxrws---`: owner and group fully, setgid, nobody else.
pub const SHARE_DIR_MODE: u32 = 0o2770;

/// The uid the export squashes clients to. `PISTACK_SHELF_SQUASH_UID`
/// overrides it; unset means [`SQUASH_UID`]. The same value is the gid: the
/// pair is nobody/nogroup, one number on every machine that has either name.
pub fn squash_owner() -> u32 {
    std::env::var("PISTACK_SHELF_SQUASH_UID")
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok())
        .unwrap_or(SQUASH_UID)
}

/// Give `dir` the squashed owner and [`SHARE_DIR_MODE`]. The mode is applied
/// even when ownership is refused: an unprivileged server that cannot chown
/// still leaves the directory setgid, and a later pass by root fixes the
/// owner. The export line squashes regardless, so nothing is skipped on a
/// machine where the chown fails.
pub async fn apply(dir: &Path) -> std::io::Result<()> {
    let _ = chown(dir, squash_owner()).await;
    chmod(dir, SHARE_DIR_MODE).await
}

async fn chown(dir: &Path, owner: u32) -> std::io::Result<()> {
    let raw = dir.to_path_buf();
    tokio::task::spawn_blocking(move || raw_chown(&raw, owner))
        .await
        .map_err(std::io::Error::other)?
}

async fn chmod(dir: &Path, mode: u32) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let permissions = std::fs::Permissions::from_mode(mode);
    tokio::fs::set_permissions(dir, permissions).await
}

#[cfg(unix)]
fn raw_chown(dir: &Path, owner: u32) -> std::io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    // EINVAL: no interior NUL may reach lchown, which is how the share path
    // checks already refuse a name that would escape the shelf root.
    const EINVAL: i32 = 22;

    CString::new(dir.as_os_str().as_bytes())
        .map_err(|_| std::io::Error::from_raw_os_error(EINVAL))?;
    // `None` leaves the gid alone: on Linux a root server owns the file
    // after this and the export line's anongid decides what a client sees;
    // BSDs refuse a chown into a group the process is not a member of, and
    // refusing that is what leaves the gid to the owner.
    std::os::unix::fs::lchown(dir, Some(owner), None)
}

#[cfg(not(unix))]
fn raw_chown(_dir: &Path, _owner: u32) -> std::io::Result<()> {
    Err(std::io::Error::last_os_error())
}

/// The owner and mode `dir` has right now. Test-only: a reconcile that
/// finds a directory wrong fixes it by applying the right values, not by
/// reading the old ones.
#[cfg(test)]
pub fn probe(dir: &Path) -> std::io::Result<(u32, u32)> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::symlink_metadata(dir)?;
    Ok((meta.uid(), meta.mode() & 0o7777))
}

/// The process's effective uid, for tests that cannot chown.
#[cfg(all(test, unix))]
pub fn euid() -> u32 {
    extern "C" {
        fn geteuid() -> std::ffi::c_int;
    }
    // SAFETY: geteuid has no preconditions and does not touch Rust memory.
    unsafe { geteuid() }.try_into().unwrap_or(u32::MAX)
}

/// One lock for every unit test that touches the process environment.
#[cfg(test)]
pub fn test_env_lock() -> &'static std::sync::Mutex<()> {
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    /// The whole unit-test binary shares this: `PISTACK_SHELF_SQUASH_UID` is
    /// read by the export line and by the directory owner alike.
    fn env_lock() -> &'static std::sync::Mutex<()> {
        super::test_env_lock()
    }

    #[test]
    fn squash_owner_defaults_to_nobody_and_takes_the_override() {
        let _guard = env_lock().lock().unwrap();
        let prev = std::env::var_os("PISTACK_SHELF_SQUASH_UID");
        std::env::remove_var("PISTACK_SHELF_SQUASH_UID");
        assert_eq!(squash_owner(), SQUASH_UID);
        std::env::set_var("PISTACK_SHELF_SQUASH_UID", "12345");
        assert_eq!(squash_owner(), 12_345);
        std::env::set_var("PISTACK_SHELF_SQUASH_UID", " 7 ");
        assert_eq!(squash_owner(), 7);
        std::env::set_var("PISTACK_SHELF_SQUASH_UID", "not-a-uid");
        assert_eq!(squash_owner(), SQUASH_UID);
        match prev {
            Some(v) => std::env::set_var("PISTACK_SHELF_SQUASH_UID", v),
            None => std::env::remove_var("PISTACK_SHELF_SQUASH_UID"),
        }
    }

    #[tokio::test]
    async fn apply_sets_the_mode_and_the_owner_a_server_can_reach() {
        let me = euid();
        // The environment is set and restored around the await: the
        // process-wide uid is read by `apply`, and no other test of this
        // crate runs while this one holds it.
        let prev = std::env::var_os("PISTACK_SHELF_SQUASH_UID");
        std::env::set_var("PISTACK_SHELF_SQUASH_UID", me.to_string());
        let dir = tempfile::tempdir().expect("tempdir");
        // The uid this process certainly has: the chown succeeds without
        // root, and the mode check runs either way.
        let outcome = apply(dir.path()).await;
        match prev {
            Some(v) => std::env::set_var("PISTACK_SHELF_SQUASH_UID", v),
            None => std::env::remove_var("PISTACK_SHELF_SQUASH_UID"),
        }
        outcome.expect("apply");
        let (uid, mode) = probe(dir.path()).expect("probe");
        assert_eq!(uid, me, "dir uid {uid}");
        assert_eq!(mode, SHARE_DIR_MODE, "{mode:#o}");
    }
}
