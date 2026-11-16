// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_store::Store;
use std::sync::atomic::Ordering;

use super::test_support::*;
use crate::backend::{migrate_to_vault, rewrap};
use crate::vault::mock::{MockVault, KEY, TOKEN};

#[test]
fn rewrap_refusals() {
    let vault = MockVault::start();
    let v = vault.config(TOKEN, KEY);
    let store = Store::in_memory().unwrap();
    assert!(rewrap(&store, &v)
        .unwrap_err()
        .to_string()
        .contains("not wrapped"));
    software_cloud(&store);
    assert!(rewrap(&store, &v)
        .unwrap_err()
        .to_string()
        .contains("not wrapped"));
    migrate_to_vault(&store, &v).unwrap();
    let before = state(&store);
    let e = rewrap(&store, &vault.config(TOKEN, "other"))
        .unwrap_err()
        .to_string();
    assert!(e.contains("wrapped with Transit mount"), "{e}");
    vault.rotate();
    vault.tamper_rewrap.store(true, Ordering::SeqCst);
    let e = rewrap(&store, &v).unwrap_err().to_string();
    assert!(e.contains("different key"), "{e}");
    vault.tamper_rewrap.store(false, Ordering::SeqCst);
    let e = rewrap(&store, &vault.config("s.wrong", KEY))
        .unwrap_err()
        .to_string();
    assert!(e.contains("permission denied"), "{e}");
    assert_eq!(state(&store), before);
}
