// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use chrono::Utc;
use pistack_store::Store;

use super::test_support::*;
use crate::backend::consts::*;
use crate::backend::forget_unwrapped;
use crate::backend::wrapped_root_key::WrappedRootKey;
use crate::backend::{migrate_to_software, migrate_to_vault, KeyBackend};
use crate::crypto::RootKey;
use crate::vault::mock::{MockVault, KEY, TOKEN};

#[test]
fn two_different_keys_are_never_resolved_automatically() {
    let vault = MockVault::start();
    let v = vault.config(TOKEN, KEY);
    let store = Store::in_memory().unwrap();
    software_cloud(&store);
    let other = RootKey::generate();
    store
        .put_meta(
            META_ROOT_KEY_WRAPPED,
            &WrappedRootKey {
                backend: WRAPPED_BY_VAULT.into(),
                mount: "transit".into(),
                key: KEY.into(),
                ciphertext: v.encrypt(other.bytes()).unwrap(),
                created_at: Utc::now(),
                rewrapped_at: None,
            },
        )
        .unwrap();
    let before = state(&store);
    forget_unwrapped();
    let e = KeyBackend::VaultTransit(v.clone())
        .load_existing(&store)
        .unwrap_err()
        .to_string();
    assert!(e.contains("two different root keys"), "{e}");
    for r in [
        migrate_to_vault(&store, &v),
        migrate_to_software(&store, &v),
    ] {
        let e = r.unwrap_err().to_string();
        assert!(e.contains("two different root keys"), "{e}");
    }
    assert_eq!(state(&store), before);
    assert!(store.get_meta::<String>(META_ROOT_KEY).unwrap().is_some());
}
