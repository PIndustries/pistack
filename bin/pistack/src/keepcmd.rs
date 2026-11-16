// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `pistack keep …`: root-key operations on this node's store.
//!
//! They run with the server stopped: a running server holds the store file
//! open and the open here fails. They read the same `PISTACK_KEEP_*`
//! environment as the server. Each command changes only this node's copy
//! of the store, so a cluster needs it run on every control-plane node.
//! The work itself is in `keep::backend`.

use anyhow::{bail, Context};
use keep::backend::{self, KeyBackend, RootKeyState, ENV_BACKEND};
use keep::vault::VaultTransit;
use pistack_store::Store;
use std::path::Path;
use tokio::sync::broadcast;

fn open_store(data_dir: &Path) -> anyhow::Result<Store> {
    let path = data_dir.join("state.redb");
    if !path.exists() {
        bail!("no store at {}; nothing to do", path.display());
    }
    let (events, _) = broadcast::channel(1);
    Store::open(&path, events).with_context(|| {
        format!(
            "opening {} (is a pistack server still running on it? stop it first)",
            path.display()
        )
    })
}

fn vault_settings(what: &str) -> anyhow::Result<VaultTransit> {
    match KeyBackend::from_env()? {
        KeyBackend::VaultTransit(v) => Ok(v),
        KeyBackend::Software => bail!(
            "{what} needs {ENV_BACKEND}=vault and the PISTACK_KEEP_VAULT_* settings \
             (the same ones the server uses)"
        ),
    }
}

/// Each node has its own copy of the root key; remind the operator.
fn cluster_note(store: &Store) {
    let members: Vec<serde_json::Value> =
        store.get_meta("members").ok().flatten().unwrap_or_default();
    if members.len() > 1 {
        eprintln!(
            "note: this node is one of {} control-plane members; each holds its own copy of \
             the root key, so run the same command on every member (servers stopped) with \
             the same Vault settings",
            members.len()
        );
    }
}

pub(crate) fn describe(state: &RootKeyState) -> String {
    match state {
        RootKeyState::Empty => "none (the first server start creates one)".into(),
        RootKeyState::Software => "software: keep/root_key, in the clear".into(),
        RootKeyState::Wrapped(w) => format!(
            "vault-transit: mount '{}', key '{}', key version v{}, wrapped {}{}",
            w.mount,
            w.key,
            w.key_version().unwrap_or(0),
            w.created_at.to_rfc3339(),
            w.rewrapped_at
                .map(|t| format!(", rewrapped {}", t.to_rfc3339()))
                .unwrap_or_default()
        ),
        RootKeyState::Both(w) => format!(
            "BOTH a software key and a vault-transit key (mount '{}', key '{}'); the server \
             refuses to start until `pistack keep migrate-to-vault` or \
             `pistack keep migrate-to-software` settles it",
            w.mount, w.key
        ),
    }
}

pub fn status(data_dir: &Path) -> anyhow::Result<()> {
    let store = open_store(data_dir)?;
    let state = backend::root_key_state(&store)?;
    println!("root key: {}", describe(&state));
    let configured = KeyBackend::from_env()?;
    println!("configured backend: {}", configured.name());
    if state == RootKeyState::Empty {
        return Ok(());
    }
    match keep::rekey::rotated_at(&store)? {
        Some(at) => println!("root key last rotated: {}", at.to_rfc3339()),
        None => println!("root key last rotated: never"),
    }
    if keep::rekey::in_progress(&store)? {
        println!(
            "a root key rotation is UNFINISHED; run `pistack keep rotate-root-key` again \
             (with the same --key-file, if one was used) to finish it"
        );
    }
    // Load it exactly as the server would, so this says what startup will say.
    configured
        .load_existing(&store)
        .context("the server would refuse to start")?;
    println!("check: the configured backend opens this root key");
    Ok(())
}

pub fn migrate_to_vault(data_dir: &Path) -> anyhow::Result<()> {
    let v = vault_settings("migrate-to-vault")?;
    let store = open_store(data_dir)?;
    cluster_note(&store);
    let m = backend::migrate_to_vault(&store, &v)?;
    if m.resumed {
        println!("the store already held this key wrapped by Vault; removed the plaintext copy");
    } else {
        println!(
            "root key wrapped by Vault Transit (mount '{}', key '{}', v{}), checked, and the \
             plaintext copy removed in the same transaction",
            m.mount,
            m.key,
            m.key_version.unwrap_or(0)
        );
    }
    println!(
        "start the server with {ENV_BACKEND}=vault and these settings. Store backups taken \
         before now still hold the key in the clear: destroy them or guard them like the key."
    );
    Ok(())
}

pub fn migrate_to_software(data_dir: &Path, allow_plaintext_key: bool) -> anyhow::Result<()> {
    if !allow_plaintext_key {
        bail!(
            "this writes the root key into the store in the clear, so anyone with the store \
             file can open every secret; pass --allow-plaintext-key to do it anyway"
        );
    }
    let v = vault_settings("migrate-to-software")?;
    let store = open_store(data_dir)?;
    cluster_note(&store);
    let m = backend::migrate_to_software(&store, &v)?;
    if m.resumed {
        println!("the store already held this key in the clear; removed the wrapped copy");
    } else {
        println!(
            "root key unwrapped from Vault Transit (mount '{}', key '{}') and stored in the \
             clear; the wrapped copy was removed in the same transaction",
            m.mount, m.key
        );
    }
    println!(
        "unset {ENV_BACKEND} and the PISTACK_KEEP_VAULT_* settings before starting the server"
    );
    Ok(())
}

pub fn rewrap(data_dir: &Path) -> anyhow::Result<()> {
    let v = vault_settings("rewrap")?;
    let store = open_store(data_dir)?;
    cluster_note(&store);
    let r = backend::rewrap(&store, &v)?;
    let (from, to) = (r.from_version.unwrap_or(0), r.to_version.unwrap_or(0));
    if r.changed {
        println!("root key rewrapped from Transit key version v{from} to v{to}");
    } else {
        println!("root key is already on the latest Transit key version (v{to}); nothing written");
    }
    Ok(())
}

/// The key in `path`, or a new one written there when the file does not
/// exist yet. The file is created private and never overwritten.
fn key_from_file(path: &Path) -> anyhow::Result<keep::RootKey> {
    if path.exists() {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading the key file {}", path.display()))?;
        return keep::rekey::decode(&text)
            .with_context(|| format!("the key file {} does not hold a root key", path.display()));
    }
    let encoded = keep::rekey::generate_encoded();
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
            .with_context(|| format!("writing the key file {}", path.display()))?;
        writeln!(file, "{encoded}")?;
    }
    eprintln!("wrote a new root key to {} (mode 0600)", path.display());
    Ok(keep::rekey::decode(&encoded)?)
}

pub fn rotate_root_key(data_dir: &Path, key_file: Option<&Path>) -> anyhow::Result<()> {
    if !matches!(KeyBackend::from_env()?, KeyBackend::Software) {
        bail!(
            "rotate-root-key seals the secrets again under a new software root key. With \
             {ENV_BACKEND}=vault the root key is wrapped by Transit: rotate the Transit key in \
             Vault and run `pistack keep rewrap`"
        );
    }
    let store = open_store(data_dir)?;
    let members: Vec<serde_json::Value> =
        store.get_meta("members").ok().flatten().unwrap_or_default();
    let resuming = keep::rekey::in_progress(&store)?;
    if members.len() > 1 && key_file.is_none() && !resuming {
        bail!(
            "this node is one of {} control-plane members and each holds its own copy of the \
             store, so every member must be given the same new key. Stop all servers, run \
             `pistack keep rotate-root-key --key-file PATH` here (it writes a new key to PATH), \
             copy that file to the other members and run the same command on each, then \
             delete the file everywhere",
            members.len()
        );
    }
    // Read or made before the store is touched, so a bad file changes nothing.
    let key = key_file.map(key_from_file).transpose()?;
    let r = keep::rekey::rotate_root_key(&store, key)?;
    if r.resumed {
        println!("finished a rotation that was started earlier");
    }
    println!(
        "root key rotated: {} payload(s) sealed again, {} already under the new key",
        r.resealed, r.already
    );
    if members.len() > 1 {
        println!(
            "run the same command with the same key file on the other {} member(s) before \
             starting any server, then delete the key file",
            members.len() - 1
        );
    }
    println!(
        "store backups taken before now are sealed with the old key and hold that key: keep \
         them only as long as you need them"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_file_is_made_once_and_read_back_the_same() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("new-root.key");
        let made = key_from_file(&path).unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        // What the next server reads is the key the first one made.
        let read = key_from_file(&path).unwrap();
        let probe = made.seal(b"x", b"aad").unwrap();
        assert_eq!(read.open(&probe, b"aad").unwrap(), b"x");

        std::fs::write(&path, "not a key").unwrap();
        let err = key_from_file(&path).unwrap_err();
        assert!(
            format!("{err:#}").contains("does not hold a root key"),
            "{err:#}"
        );
    }

    #[test]
    fn two_servers_given_one_key_file_end_on_the_same_key() {
        let dir = tempfile::tempdir().unwrap();
        let key_file = dir.path().join("new-root.key");
        let mut keys = Vec::new();
        for node in ["a", "b"] {
            let data = dir.path().join(node);
            std::fs::create_dir_all(&data).unwrap();
            let secret = {
                let (events, _) = broadcast::channel(1);
                let store = Store::open(data.join("state.redb"), events).unwrap();
                let c = keep::Client::open_with(&store, &KeyBackend::Software).unwrap();
                let s = keep::Secret::new("db", None, keep::SecretSpec::default());
                c.create_secret(s, b"pw").unwrap().meta.id
            };
            rotate_root_key(&data, Some(&key_file)).unwrap();
            let (events, _) = broadcast::channel(1);
            let store = Store::open(data.join("state.redb"), events).unwrap();
            let c = keep::Client::open_with(&store, &KeyBackend::Software).unwrap();
            assert_eq!(c.get_secret_payload(&secret).unwrap(), b"pw");
            keys.push(store.get_meta::<String>("keep/root_key").unwrap().unwrap());
        }
        assert_eq!(keys[0], keys[1]);
        assert_eq!(keys[0], std::fs::read_to_string(&key_file).unwrap().trim());
    }
}
