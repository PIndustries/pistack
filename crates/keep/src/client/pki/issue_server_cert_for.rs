// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use crate::error::Result;
use crate::tls::Pki;
use crate::types::CertUsage;

impl Client {
    /// [`issue_server_cert`](Self::issue_server_cert) for a key the caller
    /// made and shows in `csr_pem`. The returned `key_pem` is empty.
    pub fn issue_server_cert_for(&self, cn: &str, sans: &[String], csr_pem: &str) -> Result<Pki> {
        self.issue_pki(cn, sans.to_vec(), CertUsage::Server, Some(csr_pem))
    }
}
