// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_store::Store;
use std::sync::atomic::Ordering;

use super::test_support::*;
use crate::backend::root_key_state::RootKeyState;
use crate::backend::{migrate_to_vault, KeyBackend};
use crate::vault::mock::{MockVault, KEY, TOKEN};

#[test]
fn migrate_to_vault_refusals_leave_the_software_key_alone() {
    let vault = MockVault::start();
    let store = Store::in_memory().unwrap();
    // Nothing to migrate in an empty store.
    let e = migrate_to_vault(&store, &vault.config(TOKEN, KEY))
        .unwrap_err()
        .to_string();
    assert!(e.contains("no root key"), "{e}");
    assert_eq!(state(&store), RootKeyState::Empty);

    let (key, secret, ca) = software_cloud(&store);
    let unchanged = |store: &Store| {
        assert_eq!(state(store), RootKeyState::Software);
        reopen_and_check(store, &KeyBackend::Software, &secret, &ca);
        let k = KeyBackend::Software.load_existing(store).unwrap().unwrap();
        assert_eq!(k.bytes(), key.bytes());
    };
    // Vault refuses the token.
    let e = migrate_to_vault(&store, &vault.config("s.wrong", KEY))
        .unwrap_err()
        .to_string();
    assert!(e.contains("permission denied"), "{e}");
    unchanged(&store);
    // Vault is down.
    vault.down.store(true, Ordering::SeqCst);
    let e = migrate_to_vault(&store, &vault.config(TOKEN, KEY))
        .unwrap_err()
        .to_string();
    assert!(e.contains("sealed"), "{e}");
    vault.down.store(false, Ordering::SeqCst);
    unchanged(&store);
    // Vault unwraps to something else: the round-trip check catches it.
    vault.tamper.store(true, Ordering::SeqCst);
    let e = migrate_to_vault(&store, &vault.config(TOKEN, KEY))
        .unwrap_err()
        .to_string();
    assert!(e.contains("different bytes"), "{e}");
    vault.tamper.store(false, Ordering::SeqCst);
    unchanged(&store);
    // Startup with vault settings explains what to run.
    let e = KeyBackend::VaultTransit(vault.config(TOKEN, KEY))
        .load_or_create(&store)
        .unwrap_err()
        .to_string();
    assert!(e.contains("pistack keep migrate-to-vault"), "{e}");
    unchanged(&store);
}
