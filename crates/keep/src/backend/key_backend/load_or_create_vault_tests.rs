// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use base64::{engine::general_purpose::STANDARD as B64, Engine};
use pistack_store::Store;

use crate::backend::consts::*;
use crate::backend::forget_unwrapped;
use crate::backend::wrapped_root_key::WrappedRootKey;
use crate::backend::KeyBackend;
use crate::vault::mock::{MockVault, KEY, TOKEN};

#[test]
fn vault_backend_wraps_and_unwraps_the_root_key() {
    let vault = MockVault::start();
    let backend = KeyBackend::VaultTransit(vault.config(TOKEN, KEY));
    let store = Store::in_memory().unwrap();
    assert!(backend.load_existing(&store).unwrap().is_none());

    let key = backend.load_or_create(&store).unwrap();
    assert_eq!(vault.encrypts.load(std::sync::atomic::Ordering::SeqCst), 1);
    // Nothing in the clear: no software key, and the wrapped blob does
    // not contain the key bytes.
    assert!(store.get_meta::<String>(META_ROOT_KEY).unwrap().is_none());
    let w: WrappedRootKey = store.get_meta(META_ROOT_KEY_WRAPPED).unwrap().unwrap();
    assert!(w.ciphertext.starts_with("vault:v1:"));
    assert!(!w.ciphertext.contains(&B64.encode(key.bytes())));
    assert_eq!((w.mount.as_str(), w.key.as_str()), ("transit", KEY));

    let sealed = key.seal(b"payload", b"sec-1").unwrap();

    // A fresh process (empty cache) unwraps through Vault and opens it.
    forget_unwrapped();
    let before = vault.decrypts.load(std::sync::atomic::Ordering::SeqCst);
    let again = backend.load_existing(&store).unwrap().unwrap();
    assert!(vault.decrypts.load(std::sync::atomic::Ordering::SeqCst) > before);
    assert_eq!(again.open(&sealed, b"sec-1").unwrap(), b"payload");
    let same = backend.load_or_create(&store).unwrap();
    assert_eq!(same.bytes(), key.bytes());
    assert_eq!(vault.encrypts.load(std::sync::atomic::Ordering::SeqCst), 1);
}
