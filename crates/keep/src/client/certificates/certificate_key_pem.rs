// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use crate::error::KeepError;
use crate::error::Result;
use crate::types::Certificate;

impl Client {
    /// Decrypted private key PEM of a certificate.
    pub fn certificate_key_pem(&self, cert: &Certificate) -> Result<String> {
        let id = cert.status.key_secret.as_ref().ok_or_else(|| {
            KeepError::NotFound(format!("certificate {} has no key", cert.meta.id))
        })?;
        let bytes = self.get_secret_payload(id)?;
        String::from_utf8(bytes).map_err(|_| KeepError::Pki("key is not PEM text".into()))
    }
}
