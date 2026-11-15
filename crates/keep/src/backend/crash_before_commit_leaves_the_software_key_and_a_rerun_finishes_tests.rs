// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::sync::atomic::Ordering;

use super::test_support::*;
use crate::backend::forget_unwrapped;
use crate::backend::root_key_state::RootKeyState;
use crate::backend::{migrate_to_vault, KeyBackend};
use crate::vault::mock::{MockVault, KEY, TOKEN};

#[test]
fn crash_before_commit_leaves_the_software_key_and_a_rerun_finishes() {
    let vault = MockVault::start();
    let v = vault.config(TOKEN, KEY);
    let (store, crashy) = crashy_store();
    let (key, secret, ca) = software_cloud(&store);

    crashy.crash.store(true, Ordering::SeqCst);
    let e = migrate_to_vault(&store, &v).unwrap_err().to_string();
    assert!(e.contains("simulated crash"), "{e}");
    crashy.crash.store(false, Ordering::SeqCst);
    // Vault was asked (wrap + verify), but the store never saw a write:
    // still exactly the software key, never neither and never both.
    assert_eq!(vault.encrypts.load(Ordering::SeqCst), 1);
    assert_eq!(state(&store), RootKeyState::Software);
    reopen_and_check(&store, &KeyBackend::Software, &secret, &ca);

    // Running it again completes.
    migrate_to_vault(&store, &v).unwrap();
    assert!(matches!(state(&store), RootKeyState::Wrapped(_)));
    let vb = KeyBackend::VaultTransit(v);
    reopen_and_check(&store, &vb, &secret, &ca);
    forget_unwrapped();
    assert_eq!(
        vb.load_existing(&store).unwrap().unwrap().bytes(),
        key.bytes()
    );
}
