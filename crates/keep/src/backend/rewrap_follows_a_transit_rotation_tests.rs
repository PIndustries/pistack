// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_store::Store;

use super::test_support::*;
use crate::backend::forget_unwrapped;
use crate::backend::rewrapped::Rewrapped;
use crate::backend::root_key_state::RootKeyState;
use crate::backend::{migrate_to_vault, rewrap, KeyBackend};
use crate::vault::mock::{MockVault, KEY, TOKEN};

#[test]
fn rewrap_follows_a_transit_rotation() {
    let vault = MockVault::start();
    let v = vault.config(TOKEN, KEY);
    let vb = KeyBackend::VaultTransit(v.clone());
    let store = Store::in_memory().unwrap();
    let (key, secret, ca) = software_cloud(&store);
    migrate_to_vault(&store, &v).unwrap();
    let RootKeyState::Wrapped(w1) = state(&store) else {
        panic!()
    };

    // Already on the latest version: reported, nothing written.
    let r = rewrap(&store, &v).unwrap();
    assert_eq!(
        r,
        Rewrapped {
            from_version: Some(1),
            to_version: Some(1),
            changed: false
        }
    );
    assert_eq!(state(&store), RootKeyState::Wrapped(w1.clone()));

    vault.rotate();
    let r = rewrap(&store, &v).unwrap();
    assert_eq!(
        (r.from_version, r.to_version, r.changed),
        (Some(1), Some(2), true)
    );
    let RootKeyState::Wrapped(w2) = state(&store) else {
        panic!()
    };
    assert!(w2.ciphertext.starts_with("vault:v2:"));
    assert_eq!(w2.created_at, w1.created_at);
    assert!(w2.rewrapped_at.is_some());

    // Now the old version can be retired without locking keep out.
    vault.set_min_decryption_version(2);
    forget_unwrapped();
    assert_eq!(
        vb.load_existing(&store).unwrap().unwrap().bytes(),
        key.bytes()
    );
    reopen_and_check(&store, &vb, &secret, &ca);
}
