// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_store::Store;

use crate::backend::consts::*;
use crate::backend::wrapped_root_key::WrappedRootKey;
use crate::backend::KeyBackend;
use crate::vault::mock::{MockVault, KEY, TOKEN};

#[test]
fn vault_backend_fails_closed() {
    let vault = MockVault::start();
    let store = Store::in_memory().unwrap();

    // Wrong token on first start: nothing is written.
    let bad = KeyBackend::VaultTransit(vault.config("s.wrong", KEY));
    let e = bad.load_or_create(&store).unwrap_err().to_string();
    assert!(e.contains("permission denied"), "{e}");
    assert!(store.get_meta::<String>(META_ROOT_KEY).unwrap().is_none());
    assert!(store
        .get_meta::<WrappedRootKey>(META_ROOT_KEY_WRAPPED)
        .unwrap()
        .is_none());

    // Create with the right token, then a wrong token cannot unwrap even
    // though this process has the key cached for the right one.
    let good = KeyBackend::VaultTransit(vault.config(TOKEN, KEY));
    good.load_or_create(&store).unwrap();
    let e = bad.load_existing(&store).unwrap_err().to_string();
    assert!(e.contains("permission denied"), "{e}");

    // Another Transit key name is refused before Vault is asked.
    let other = KeyBackend::VaultTransit(vault.config(TOKEN, "other"));
    let e = other.load_existing(&store).unwrap_err().to_string();
    assert!(e.contains("wrapped with Transit mount"), "{e}");

    // The software backend will not touch a Vault-wrapped store.
    let e = KeyBackend::Software
        .load_or_create(&store)
        .unwrap_err()
        .to_string();
    assert!(e.contains("wrapped by Vault Transit"), "{e}");
    assert!(store.get_meta::<String>(META_ROOT_KEY).unwrap().is_none());
}
