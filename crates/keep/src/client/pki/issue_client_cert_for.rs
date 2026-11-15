// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use crate::error::Result;
use crate::tls::Pki;
use crate::types::CertUsage;

impl Client {
    /// [`issue_client_cert`](Self::issue_client_cert) for a key the caller
    /// made and shows in `csr_pem`. The returned `key_pem` is empty.
    pub fn issue_client_cert_for(&self, cn: &str, csr_pem: &str) -> Result<Pki> {
        self.issue_pki(cn, Vec::new(), CertUsage::Client, Some(csr_pem))
    }
}
