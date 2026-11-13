// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `pistack store restore`: put a backup in a data directory.
//!
//! It runs with no server on the data directory. The backup is copied into
//! the directory under another name and everything is checked on that copy:
//! it is a whole store, this binary can read its schema version, and it is
//! the store of a server that has run. Only then is the old store renamed
//! and the copy moved into place, so a backup that fails a check costs
//! nothing but the copy, which is removed. The backup file is never opened
//! for writing.
//!
//! A restore founds the cluster again with one member. The backup names the
//! servers of the cluster it came from, and none of them holds this state
//! any more, so the membership is dropped (see
//! [`crate::cluster::forget_membership`]). The other servers come back by
//! joining with an empty data directory.
//!
//! What else is in the data directory:
//!
//! * `admin-password` is renamed with the old store. It was written when
//!   that store was founded and the restored store has its own admin
//!   password.
//! * `adopt.url`, `state.partial` and `state.resync` are left over from a
//!   join or a store replacement that was under way. They are removed.
//! * `node.id` and `node.name` stay. The server seeds the membership with
//!   whatever id it finds, so the same machine keeps its id and a new
//!   machine makes one.
//! * `tls/`, `agent/` and the module directories stay and are not looked
//!   at. The root key is in the store, in the clear or wrapped by Vault,
//!   and comes back as it was in the backup.

use anyhow::{bail, Context};
use pistack_store::Store;
use std::path::{Path, PathBuf};
use tokio::sync::broadcast;

const STORE_FILE: &str = "state.redb";
/// The backup while it is being checked, before it becomes `state.redb`.
const INCOMING_FILE: &str = "state.restore";
/// Renamed with the old store.
const SET_ASIDE: [&str; 2] = [STORE_FILE, gate::ADMIN_PASSWORD_FILE];
/// Left over from a join or a store replacement; they describe the old
/// store.
const REMOVED: [&str; 3] = ["adopt.url", "state.partial", "state.resync"];

#[derive(Debug)]
pub struct Restored {
    pub cluster_id: String,
    /// The schema version in the backup, and in the restored store after
    /// any migration this binary ran on it.
    pub backup_schema: u32,
    pub schema: u32,
    /// Servers the backup named as members.
    pub members_in_backup: usize,
    pub root_key: String,
    pub set_aside: Vec<PathBuf>,
    pub removed: Vec<PathBuf>,
    /// What was in the data directory and was not touched.
    pub left: Vec<String>,
}

pub fn run(data_dir: &Path, from: &Path, force: bool) -> anyhow::Result<()> {
    let done = restore(data_dir, from, force)?;
    println!(
        "restored {} to {}",
        from.display(),
        data_dir.join(STORE_FILE).display()
    );
    println!("cluster: {}", done.cluster_id);
    if done.schema == done.backup_schema {
        println!("store schema version: {}", done.schema);
    } else {
        println!(
            "store schema version: {} in the backup, migrated to {}",
            done.backup_schema, done.schema
        );
    }
    println!("root key: {}", done.root_key);
    for path in &done.set_aside {
        println!("set aside: {}", path.display());
    }
    for path in &done.removed {
        println!("removed: {}", path.display());
    }
    if !done.left.is_empty() {
        println!("left as it was: {}", done.left.join(", "));
    }
    println!(
        "the backup named {} server(s). The server started on this directory will be the only \
         member and the leader. Do not start any other server of the old cluster on its old \
         store: stop it, empty its data directory and start it with --join",
        done.members_in_backup
    );
    Ok(())
}

pub fn restore(data_dir: &Path, from: &Path, force: bool) -> anyhow::Result<Restored> {
    if !from.is_file() {
        bail!("{} is not a file", from.display());
    }
    let target = data_dir.join(STORE_FILE);
    if let (Ok(backup), Ok(store)) = (from.canonicalize(), target.canonicalize()) {
        if backup == store {
            bail!("{} is the store in the data directory", from.display());
        }
    }
    let present = entries(data_dir)?;
    if !present.is_empty() && !force {
        bail!(
            "{} is not empty (it holds {}). Restore into an empty directory, or pass --force: \
             the store and the admin password file that are there are renamed, not deleted, \
             and everything else is left as it is",
            data_dir.display(),
            present.join(", ")
        );
    }
    // A running server keeps writing to its store under whatever name.
    if target.exists() && pistack_store::is_held_open(&target) {
        bail!(
            "a pistack server has {} open; stop it first",
            target.display()
        );
    }

    let made_dir = !data_dir.exists();
    std::fs::create_dir_all(data_dir)
        .with_context(|| format!("creating {}", data_dir.display()))?;
    let incoming = data_dir.join(INCOMING_FILE);
    let checked = match prepare(from, &incoming) {
        Ok(checked) => checked,
        Err(err) => {
            let _ = std::fs::remove_file(&incoming);
            if made_dir {
                let _ = std::fs::remove_dir(data_dir);
            }
            return Err(err);
        }
    };

    // The checks are done. From here the data directory changes.
    let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    let mut set_aside = Vec::new();
    for name in SET_ASIDE {
        let path = data_dir.join(name);
        if path.exists() {
            let aside = free_name(data_dir, &format!("{name}.before-restore-{stamp}"));
            std::fs::rename(&path, &aside)
                .with_context(|| format!("renaming {} to {}", path.display(), aside.display()))?;
            set_aside.push(aside);
        }
    }
    let mut removed = Vec::new();
    for name in REMOVED {
        let path = data_dir.join(name);
        if path.exists() {
            std::fs::remove_file(&path).with_context(|| format!("removing {}", path.display()))?;
            removed.push(path);
        }
    }
    std::fs::rename(&incoming, &target).with_context(|| {
        format!(
            "moving the checked backup {} into place as {}",
            incoming.display(),
            target.display()
        )
    })?;
    if let Ok(dir) = std::fs::File::open(data_dir) {
        let _ = dir.sync_all();
    }

    let moved: Vec<String> = set_aside
        .iter()
        .filter_map(|path| path.file_name())
        .map(|name| name.to_string_lossy().into_owned())
        .collect();
    let left = entries(data_dir)?
        .into_iter()
        .filter(|name| name != STORE_FILE && !moved.contains(name))
        .collect();
    Ok(Restored {
        cluster_id: checked.cluster_id,
        backup_schema: checked.backup_schema,
        schema: checked.schema,
        members_in_backup: checked.members_in_backup,
        root_key: checked.root_key,
        set_aside,
        removed,
        left,
    })
}

struct Checked {
    cluster_id: String,
    backup_schema: u32,
    schema: u32,
    members_in_backup: usize,
    root_key: String,
}

/// Copy the backup to `incoming` and make that copy the store to start
/// from. Every check a restore makes is here; the caller removes
/// `incoming` when one fails.
fn prepare(from: &Path, incoming: &Path) -> anyhow::Result<Checked> {
    std::fs::copy(from, incoming)
        .with_context(|| format!("copying {} to {}", from.display(), incoming.display()))?;
    let info = pistack_store::check_file(incoming)
        .with_context(|| format!("{} is not a whole store", from.display()))?;
    if info.schema_version > pistack_store::SCHEMA_VERSION {
        bail!(
            "{} is at store schema version {} and this pistack reads up to {}; restore it with \
             a pistack at least as new as the one that wrote it",
            from.display(),
            info.schema_version,
            pistack_store::SCHEMA_VERSION
        );
    }
    let (events, _) = broadcast::channel(1);
    // Opening it runs the migrations an older backup needs.
    let store = Store::open(incoming, events).context("opening the copied backup")?;
    let Some(cluster_id) = store.get_meta::<String>("cluster_id")? else {
        bail!(
            "{} holds no cluster id: it is not the store of a server that has run",
            from.display()
        );
    };
    let members_in_backup = crate::cluster::forget_membership(&store)?;
    let root_key = crate::keepcmd::describe(&keep::backend::root_key_state(&store)?);
    let schema = store.schema_version()?;
    drop(store);
    std::fs::File::open(incoming)
        .and_then(|file| file.sync_all())
        .with_context(|| format!("syncing {}", incoming.display()))?;
    Ok(Checked {
        cluster_id,
        backup_schema: info.schema_version,
        schema,
        members_in_backup,
        root_key,
    })
}

/// Names in `dir`, sorted. A directory that is not there has none.
fn entries(dir: &Path) -> anyhow::Result<Vec<String>> {
    let read = match std::fs::read_dir(dir) {
        Ok(read) => read,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err).with_context(|| format!("reading {}", dir.display())),
    };
    let mut names = Vec::new();
    for entry in read {
        names.push(entry?.file_name().to_string_lossy().into_owned());
    }
    names.sort();
    Ok(names)
}

/// `dir/name`, or `dir/name-2` and so on when that is taken.
fn free_name(dir: &Path, name: &str) -> PathBuf {
    let mut path = dir.join(name);
    let mut n = 2;
    while path.exists() {
        path = dir.join(format!("{name}-{n}"));
        n += 1;
    }
    path
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cluster::{establish_local, LocalRole};
    use pistack_cluster::{Member, Role};

    fn open(path: &Path) -> Store {
        let (events, _) = broadcast::channel(8);
        Store::open(path, events).unwrap()
    }

    /// The store of server `a` in a two-server cluster, with an admin and
    /// the built-in roles in it, and a backup of it.
    fn backup_of_a_server(dir: &Path, cluster_id: &str) -> PathBuf {
        let home = dir.join(format!("home-{cluster_id}"));
        std::fs::create_dir_all(&home).unwrap();
        let store = open(&home.join(STORE_FILE));
        store
            .put_meta("cluster_id", &cluster_id.to_owned())
            .unwrap();
        store.put_meta("node_id", &"a".to_owned()).unwrap();
        let members = vec![
            Member::voter("a", "http://10.0.0.1:8443"),
            Member::voter("b", "http://10.0.0.2:8443"),
        ];
        store.put_meta("members", &members).unwrap();
        store
            .put_meta("cluster_removed", &vec!["gone".to_owned()])
            .unwrap();
        store
            .put_meta("cluster_leader_id", &"b".to_owned())
            .unwrap();
        store
            .put_meta("cluster_voted_for", &Some("b".to_owned()))
            .unwrap();
        store.put_meta("cluster_term", &4u64).unwrap();
        gate::bootstrap_with(&store, None).unwrap();
        store.put_meta("big/row", &"x".repeat(9000)).unwrap();
        drop(store);
        let backup = dir.join(format!("{cluster_id}.backup"));
        pistack_store::backup_file(&home.join(STORE_FILE), &backup).unwrap();
        backup
    }

    fn rows(store: &Store) -> Vec<(String, Vec<u8>)> {
        let mut all = store.raw_scan("resources", "").unwrap();
        all.extend(store.raw_scan("names", "").unwrap());
        all
    }

    #[test]
    fn a_backup_restores_into_an_empty_directory_as_a_one_server_cluster() {
        let dir = tempfile::tempdir().unwrap();
        let backup = backup_of_a_server(dir.path(), "c1");
        let backup_bytes = std::fs::read(&backup).unwrap();
        let before = rows(&open(&dir.path().join("home-c1").join(STORE_FILE)));
        assert!(before.len() > 4, "the source has resources in it");

        // One target does not exist yet, the other is there and empty.
        let absent = dir.path().join("absent");
        let empty = dir.path().join("empty");
        std::fs::create_dir_all(&empty).unwrap();
        for target in [&absent, &empty] {
            let done = restore(target, &backup, false).unwrap();
            assert_eq!(done.cluster_id, "c1");
            assert_eq!(done.backup_schema, pistack_store::SCHEMA_VERSION);
            assert_eq!(done.schema, pistack_store::SCHEMA_VERSION);
            assert_eq!(done.members_in_backup, 2);
            assert!(done.root_key.starts_with("none"), "{}", done.root_key);
            assert!(done.set_aside.is_empty() && done.removed.is_empty() && done.left.is_empty());
            assert_eq!(entries(target).unwrap(), [STORE_FILE]);

            let store = open(&target.join(STORE_FILE));
            assert_eq!(rows(&store), before);
            let big: Option<String> = store.get_meta("big/row").unwrap();
            assert_eq!(big.map(|v| v.len()), Some(9000));
            let id: Option<String> = store.get_meta("cluster_id").unwrap();
            assert_eq!(id.as_deref(), Some("c1"));
            assert_eq!(
                store.schema_version().unwrap(),
                pistack_store::SCHEMA_VERSION
            );

            // The old servers are gone from it and the term moved on.
            assert_eq!(store.get_meta::<Vec<Member>>("members").unwrap(), None);
            assert_eq!(store.get_meta::<String>("node_id").unwrap(), None);
            // Nor does it hold the old cluster's removed servers against anyone.
            let removed: Option<Vec<String>> = store.get_meta("cluster_removed").unwrap();
            assert_eq!(removed, None);
            assert_eq!(store.get_meta::<u64>("cluster_term").unwrap(), Some(5));

            // A server with an id the backup never saw starts on it and leads.
            let LocalRole::Ready(ready) =
                establish_local(&store, "fresh", "http://10.0.0.9:8443", None, true).unwrap()
            else {
                panic!("the restored store wants to join somebody");
            };
            assert!(ready.leader);
            assert_eq!(ready.role, Role::Voter);
            assert_eq!(ready.term, 5);
            assert_eq!(ready.members.len(), 1);
            assert_eq!(ready.members[0].id, "fresh");
        }
        assert_eq!(std::fs::read(&backup).unwrap(), backup_bytes);
    }

    /// The last copy of `needle` in the file, where a stored row is.
    fn last(bytes: &[u8], needle: &[u8]) -> usize {
        (0..bytes.len() - needle.len())
            .rev()
            .find(|at| &bytes[*at..*at + needle.len()] == needle)
            .unwrap()
    }

    #[test]
    fn a_cut_short_or_damaged_backup_is_refused_and_the_target_is_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let good = std::fs::read(backup_of_a_server(dir.path(), "c1")).unwrap();

        let cut = dir.path().join("cut.backup");
        std::fs::write(&cut, &good[..good.len() / 2]).unwrap();
        let mut bytes = good.clone();
        let at = last(&bytes, b"big/row");
        for byte in &mut bytes[at + 64..at + 2048] {
            *byte ^= 0x5a;
        }
        let damaged = dir.path().join("damaged.backup");
        std::fs::write(&damaged, &bytes).unwrap();
        let text = dir.path().join("notes.backup");
        std::fs::write(&text, b"not a store").unwrap();

        // A target with a store of its own, restored over with --force.
        let live = dir.path().join("live");
        std::fs::create_dir_all(&live).unwrap();
        let store = open(&live.join(STORE_FILE));
        store.put_meta("cluster_id", &"old".to_owned()).unwrap();
        drop(store);
        std::fs::write(live.join(gate::ADMIN_PASSWORD_FILE), "old password\n").unwrap();
        std::fs::write(live.join("adopt.url"), "http://10.0.0.2:8443").unwrap();
        let listing = entries(&live).unwrap();
        let store_bytes = std::fs::read(live.join(STORE_FILE)).unwrap();

        for bad in [&cut, &damaged, &text, &dir.path().join("missing.backup")] {
            let absent = dir.path().join("absent");
            let err = restore(&absent, bad, false).unwrap_err();
            assert!(!absent.exists(), "{}: {err:#}", bad.display());

            let err = restore(&live, bad, true).unwrap_err();
            assert_eq!(entries(&live).unwrap(), listing, "{err:#}");
            assert_eq!(std::fs::read(live.join(STORE_FILE)).unwrap(), store_bytes);
            assert_eq!(
                std::fs::read_to_string(live.join(gate::ADMIN_PASSWORD_FILE)).unwrap(),
                "old password\n"
            );
        }
        let id: Option<String> = open(&live.join(STORE_FILE)).get_meta("cluster_id").unwrap();
        assert_eq!(id.as_deref(), Some("old"));
    }

    #[test]
    fn a_directory_that_is_not_empty_needs_force_and_keeps_its_old_store() {
        let dir = tempfile::tempdir().unwrap();
        let backup = backup_of_a_server(dir.path(), "c1");
        let target = dir.path().join("target");
        std::fs::create_dir_all(target.join("tls")).unwrap();
        std::fs::write(target.join("node.id"), "this-machine").unwrap();

        // No store there, but not empty either.
        let err = restore(&target, &backup, false).unwrap_err().to_string();
        assert!(err.contains("is not empty"), "{err}");
        assert!(err.contains("--force"), "{err}");
        assert!(err.contains("node.id"), "{err}");
        assert_eq!(entries(&target).unwrap(), ["node.id", "tls"]);

        let store = open(&target.join(STORE_FILE));
        store.put_meta("cluster_id", &"old".to_owned()).unwrap();
        drop(store);
        std::fs::write(target.join(gate::ADMIN_PASSWORD_FILE), "old password\n").unwrap();
        std::fs::write(target.join("adopt.url"), "http://10.0.0.2:8443").unwrap();
        std::fs::write(target.join("state.partial"), "half a snapshot").unwrap();
        std::fs::write(target.join("state.resync"), "a whole one").unwrap();
        let listing = entries(&target).unwrap();
        let err = restore(&target, &backup, false).unwrap_err().to_string();
        assert!(err.contains("--force"), "{err}");
        assert_eq!(entries(&target).unwrap(), listing);
        let id: Option<String> = open(&target.join(STORE_FILE))
            .get_meta("cluster_id")
            .unwrap();
        assert_eq!(id.as_deref(), Some("old"));

        let done = restore(&target, &backup, true).unwrap();
        assert_eq!(done.cluster_id, "c1");
        let id: Option<String> = open(&target.join(STORE_FILE))
            .get_meta("cluster_id")
            .unwrap();
        assert_eq!(id.as_deref(), Some("c1"));

        // The old store and its password file are still there under new names.
        assert_eq!(done.set_aside.len(), 2);
        let old_store = &done.set_aside[0];
        let name = old_store.file_name().unwrap().to_string_lossy();
        assert!(name.starts_with("state.redb.before-restore-"), "{name}");
        let id: Option<String> = open(old_store).get_meta("cluster_id").unwrap();
        assert_eq!(id.as_deref(), Some("old"));
        assert_eq!(
            std::fs::read_to_string(&done.set_aside[1]).unwrap(),
            "old password\n"
        );
        assert!(!target.join(gate::ADMIN_PASSWORD_FILE).exists());

        // What a join left behind is gone; the rest was not touched.
        assert_eq!(done.removed.len(), 3);
        assert!(!target.join("adopt.url").exists());
        assert!(!target.join("state.partial").exists());
        assert!(!target.join("state.resync").exists());
        assert_eq!(done.left, ["node.id", "tls"]);
        assert_eq!(
            std::fs::read_to_string(target.join("node.id")).unwrap(),
            "this-machine"
        );
        assert!(!target.join(INCOMING_FILE).exists());

        // A second restore sets the first one aside under its own name.
        let again = restore(&target, &backup, true).unwrap();
        assert_eq!(again.set_aside.len(), 1);
        assert!(again.set_aside[0].exists() && old_store.exists());
        assert_ne!(&again.set_aside[0], old_store);
    }

    #[test]
    fn a_backup_from_a_newer_pistack_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let store = open(&home.join(STORE_FILE));
        store.put_meta("cluster_id", &"c1".to_owned()).unwrap();
        // As a later binary would leave it.
        store.put_meta("schema_version", &99u32).unwrap();
        drop(store);
        let backup = dir.path().join("newer.backup");
        let info = pistack_store::backup_file(&home.join(STORE_FILE), &backup).unwrap();
        assert_eq!(info.schema_version, 99);

        let target = dir.path().join("target");
        let err = restore(&target, &backup, false).unwrap_err().to_string();
        assert!(err.contains("schema version 99"), "{err}");
        assert!(err.contains("reads up to"), "{err}");
        assert!(!target.exists());
    }

    #[test]
    fn a_store_that_never_ran_a_server_and_a_running_target_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let blank = dir.path().join("blank.redb");
        drop(open(&blank));
        let target = dir.path().join("target");
        let err = restore(&target, &blank, false).unwrap_err().to_string();
        assert!(err.contains("no cluster id"), "{err}");
        assert!(!target.exists());

        let backup = backup_of_a_server(dir.path(), "c1");
        std::fs::create_dir_all(&target).unwrap();
        let running = open(&target.join(STORE_FILE));
        let err = restore(&target, &backup, true).unwrap_err().to_string();
        assert!(err.contains("stop it first"), "{err}");
        assert_eq!(entries(&target).unwrap(), [STORE_FILE]);
        drop(running);

        // The store in the directory is not a backup of itself.
        let err = restore(&target, &target.join(STORE_FILE), true)
            .unwrap_err()
            .to_string();
        assert!(err.contains("is the store in the data directory"), "{err}");
    }
}
