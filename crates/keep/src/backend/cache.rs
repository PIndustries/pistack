// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::OnceLock;

use crate::crypto::RootKey;

/// Unwrapped keys by (Vault, key, ciphertext, token fingerprint), so a
/// process asks Vault once rather than on every `Client::open`. A different
/// token is a different entry and must unwrap on its own.
pub(in crate::backend) fn cache() -> &'static Mutex<HashMap<String, RootKey>> {
    static CACHE: OnceLock<Mutex<HashMap<String, RootKey>>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

#[cfg(test)]
pub(crate) fn forget_unwrapped() {
    cache().lock().clear();
}
