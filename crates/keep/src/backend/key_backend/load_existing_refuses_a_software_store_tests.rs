// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_store::Store;

use crate::backend::consts::*;
use crate::backend::wrapped_root_key::WrappedRootKey;
use crate::backend::KeyBackend;
use crate::vault::mock::{MockVault, KEY, TOKEN};

#[test]
fn vault_backend_refuses_a_software_store() {
    let vault = MockVault::start();
    let store = Store::in_memory().unwrap();
    let soft = KeyBackend::Software.load_or_create(&store).unwrap();
    let backend = KeyBackend::VaultTransit(vault.config(TOKEN, KEY));
    let e = backend.load_or_create(&store).unwrap_err().to_string();
    assert!(e.contains("pistack keep migrate-to-vault"), "{e}");
    assert_eq!(vault.encrypts.load(std::sync::atomic::Ordering::SeqCst), 0);
    // The software key is untouched and still the only one.
    assert!(store
        .get_meta::<WrappedRootKey>(META_ROOT_KEY_WRAPPED)
        .unwrap()
        .is_none());
    let still = KeyBackend::Software.load_existing(&store).unwrap().unwrap();
    assert_eq!(still.bytes(), soft.bytes());
}
