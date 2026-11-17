// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The leaf certificate and its private key, both as PEM, as `keep` holds
//! them.

/// Leaf certificate and private key already stored by `keep`.
pub struct TlsMaterial {
    pub cert_pem: String,
    pub key_pem: String,
}

impl std::fmt::Debug for TlsMaterial {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TlsMaterial")
            .field("cert_pem", &self.cert_pem)
            .field("key_pem", &"<redacted>")
            .finish()
    }
}

impl Clone for TlsMaterial {
    fn clone(&self) -> Self {
        Self {
            cert_pem: self.cert_pem.clone(),
            key_pem: self.key_pem.clone(),
        }
    }
}
