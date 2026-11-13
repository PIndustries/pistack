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
