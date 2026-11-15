// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use chrono::Utc;
use pistack_store::Store;

use super::test_support::*;
use crate::backend::consts::*;
use crate::backend::forget_unwrapped;
use crate::backend::root_key_state::RootKeyState;
use crate::backend::wrapped_root_key::WrappedRootKey;
use crate::backend::{migrate_to_software, migrate_to_vault, rewrap, KeyBackend};
use crate::vault::mock::{MockVault, KEY, TOKEN};

#[test]
fn both_keys_are_detected_explained_and_settled_by_migrate() {
    let vault = MockVault::start();
    let v = vault.config(TOKEN, KEY);
    let vb = KeyBackend::VaultTransit(v.clone());
    let store = Store::in_memory().unwrap();
    let (key, secret, ca) = software_cloud(&store);
    // What a migration that wrote the wrapped key but died before
    // deleting the plaintext one would leave.
    let w = WrappedRootKey {
        backend: WRAPPED_BY_VAULT.into(),
        mount: "transit".into(),
        key: KEY.into(),
        ciphertext: v.encrypt(key.bytes()).unwrap(),
        created_at: Utc::now(),
        rewrapped_at: None,
    };
    store.put_meta(META_ROOT_KEY_WRAPPED, &w).unwrap();
    assert!(matches!(state(&store), RootKeyState::Both(_)));

    // Startup refuses with either backend and says what to do.
    forget_unwrapped();
    let e = vb.load_or_create(&store).unwrap_err().to_string();
    assert!(e.contains("same root key twice"), "{e}");
    assert!(e.contains("pistack keep migrate-to-vault"), "{e}");
    let e = KeyBackend::Software
        .load_or_create(&store)
        .unwrap_err()
        .to_string();
    assert!(e.contains("refusing to guess"), "{e}");
    assert!(e.contains("migrate-to-software"), "{e}");
    // Rewrap will not touch it either.
    assert!(rewrap(&store, &v)
        .unwrap_err()
        .to_string()
        .contains("first"));

    // migrate-to-vault finishes the job: only the wrapped key remains.
    let m = migrate_to_vault(&store, &v).unwrap();
    assert!(m.resumed);
    assert_eq!(state(&store), RootKeyState::Wrapped(w.clone()));
    reopen_and_check(&store, &vb, &secret, &ca);

    // The same state settled the other way keeps the plaintext key.
    let store2 = Store::in_memory().unwrap();
    let (key2, secret2, ca2) = software_cloud(&store2);
    let w2 = WrappedRootKey {
        ciphertext: v.encrypt(key2.bytes()).unwrap(),
        ..w.clone()
    };
    store2.put_meta(META_ROOT_KEY_WRAPPED, &w2).unwrap();
    assert!(migrate_to_software(&store2, &v).unwrap().resumed);
    assert_eq!(state(&store2), RootKeyState::Software);
    reopen_and_check(&store2, &KeyBackend::Software, &secret2, &ca2);
}
