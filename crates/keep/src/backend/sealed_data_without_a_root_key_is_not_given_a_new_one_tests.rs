// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_store::Store;
use std::sync::atomic::Ordering;

use super::test_support::*;
use crate::backend::consts::*;
use crate::backend::root_key_state::RootKeyState;
use crate::backend::KeyBackend;
use crate::vault::mock::{MockVault, KEY, TOKEN};

#[test]
fn sealed_data_without_a_root_key_is_not_given_a_new_one() {
    let store = Store::in_memory().unwrap();
    software_cloud(&store);
    store.delete_meta(META_ROOT_KEY).unwrap();
    let vault = MockVault::start();
    for b in [
        KeyBackend::Software,
        KeyBackend::VaultTransit(vault.config(TOKEN, KEY)),
    ] {
        let e = b.load_or_create(&store).unwrap_err().to_string();
        assert!(e.contains("sealed"), "{e}");
    }
    assert_eq!(state(&store), RootKeyState::Empty);
    assert_eq!(vault.encrypts.load(Ordering::SeqCst), 0);
}
