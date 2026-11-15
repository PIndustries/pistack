// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use base64::{engine::general_purpose::STANDARD as B64, Engine};
use pistack_store::Store;

use super::test_support::*;
use crate::backend::forget_unwrapped;
use crate::backend::migrated::Migrated;
use crate::backend::root_key_state::RootKeyState;
use crate::backend::{migrate_to_vault, KeyBackend};
use crate::vault::mock::{MockVault, KEY, TOKEN};

#[test]
fn migrate_to_vault_moves_the_key_in_one_step() {
    let vault = MockVault::start();
    let v = vault.config(TOKEN, KEY);
    let store = Store::in_memory().unwrap();
    let (key, secret, ca) = software_cloud(&store);

    let m = migrate_to_vault(&store, &v).unwrap();
    assert_eq!(
        m,
        Migrated {
            mount: "transit".into(),
            key: KEY.into(),
            key_version: Some(1),
            resumed: false
        }
    );
    let RootKeyState::Wrapped(w) = state(&store) else {
        panic!("expected only the wrapped key");
    };
    assert!(!w.ciphertext.contains(&B64.encode(key.bytes())));
    // Same key, now from Vault: existing secrets and the CA still open.
    let vb = KeyBackend::VaultTransit(v.clone());
    forget_unwrapped();
    assert_eq!(
        vb.load_existing(&store).unwrap().unwrap().bytes(),
        key.bytes()
    );
    reopen_and_check(&store, &vb, &secret, &ca);
    // The software backend now refuses, and a second run has nothing to do.
    assert!(KeyBackend::Software.load_existing(&store).is_err());
    let e = migrate_to_vault(&store, &v).unwrap_err().to_string();
    assert!(e.contains("already wrapped"), "{e}");
}
