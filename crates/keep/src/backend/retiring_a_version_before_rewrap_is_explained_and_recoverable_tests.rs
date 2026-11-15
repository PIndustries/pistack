// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_store::Store;

use super::test_support::*;
use crate::backend::forget_unwrapped;
use crate::backend::{migrate_to_vault, rewrap, KeyBackend};
use crate::vault::mock::{MockVault, KEY, TOKEN};

#[test]
fn retiring_a_version_before_rewrap_is_explained_and_recoverable() {
    let vault = MockVault::start();
    let v = vault.config(TOKEN, KEY);
    let vb = KeyBackend::VaultTransit(v.clone());
    let store = Store::in_memory().unwrap();
    let (_, secret, ca) = software_cloud(&store);
    migrate_to_vault(&store, &v).unwrap();
    let before = state(&store);

    vault.rotate();
    vault.set_min_decryption_version(2);
    forget_unwrapped();
    let e = vb.load_existing(&store).unwrap_err().to_string();
    assert!(e.contains("too old"), "{e}");
    assert!(e.contains("Lower min_decryption_version to 1"), "{e}");
    assert!(e.contains("pistack keep rewrap"), "{e}");
    // Rewrap cannot help while the version is retired, and writes nothing.
    let e = rewrap(&store, &v).unwrap_err().to_string();
    assert!(e.contains("min_decryption_version"), "{e}");
    assert_eq!(state(&store), before);

    // The documented way out.
    vault.set_min_decryption_version(1);
    assert!(rewrap(&store, &v).unwrap().changed);
    vault.set_min_decryption_version(2);
    reopen_and_check(&store, &vb, &secret, &ca);
}
