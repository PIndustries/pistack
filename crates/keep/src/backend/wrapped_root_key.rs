// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::vault::ciphertext_version;

/// The root key as Vault Transit returned it, persisted in store meta.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct WrappedRootKey {
    /// Always `vault-transit` today.
    pub backend: String,
    pub mount: String,
    pub key: String,
    /// `vault:v<n>:...`
    pub ciphertext: String,
    pub created_at: DateTime<Utc>,
    /// Set by [`rewrap`](super::rewrap).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rewrapped_at: Option<DateTime<Utc>>,
}

impl WrappedRootKey {
    /// Transit key version the ciphertext was made with (`vault:v<n>:`).
    pub fn key_version(&self) -> Option<u32> {
        ciphertext_version(&self.ciphertext)
    }
}
