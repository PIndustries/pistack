// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_store::Store;

use super::test_support::*;
use crate::backend::root_key_state::RootKeyState;
use crate::backend::{migrate_to_software, migrate_to_vault, KeyBackend};
use crate::vault::mock::{MockVault, KEY, TOKEN};

#[test]
fn migrate_to_software_reverses_it() {
    let vault = MockVault::start();
    let v = vault.config(TOKEN, KEY);
    let store = Store::in_memory().unwrap();
    let (key, secret, ca) = software_cloud(&store);
    migrate_to_vault(&store, &v).unwrap();

    // A wrong token changes nothing.
    let e = migrate_to_software(&store, &vault.config("s.wrong", KEY))
        .unwrap_err()
        .to_string();
    assert!(e.contains("permission denied"), "{e}");
    assert!(matches!(state(&store), RootKeyState::Wrapped(_)));

    let m = migrate_to_software(&store, &v).unwrap();
    assert!(!m.resumed);
    assert_eq!(state(&store), RootKeyState::Software);
    let k = KeyBackend::Software.load_existing(&store).unwrap().unwrap();
    assert_eq!(k.bytes(), key.bytes());
    reopen_and_check(&store, &KeyBackend::Software, &secret, &ca);
    let e = migrate_to_software(&store, &v).unwrap_err().to_string();
    assert!(e.contains("not wrapped"), "{e}");
}
