// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::vault::VaultTransit;

/// How the root key is kept at rest.
#[derive(Clone, Debug)]
pub enum KeyBackend {
    /// Plain base64 in store meta. Anyone with the store file has the key.
    Software,
    /// Wrapped by a Vault Transit key; only the ciphertext is stored.
    VaultTransit(VaultTransit),
}
