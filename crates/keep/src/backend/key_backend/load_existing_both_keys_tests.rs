// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use chrono::Utc;
use pistack_store::Store;

use crate::backend::consts::*;
use crate::backend::wrapped_root_key::WrappedRootKey;
use crate::backend::KeyBackend;
use crate::vault::mock::KEY;

#[test]
fn both_keys_present_is_an_error() {
    let store = Store::in_memory().unwrap();
    KeyBackend::Software.load_or_create(&store).unwrap();
    store
        .put_meta(
            META_ROOT_KEY_WRAPPED,
            &WrappedRootKey {
                backend: WRAPPED_BY_VAULT.into(),
                mount: "transit".into(),
                key: KEY.into(),
                ciphertext: "vault:v1:x".into(),
                created_at: Utc::now(),
                rewrapped_at: None,
            },
        )
        .unwrap();
    let e = KeyBackend::Software
        .load_existing(&store)
        .unwrap_err()
        .to_string();
    assert!(e.contains("both"), "{e}");
}
