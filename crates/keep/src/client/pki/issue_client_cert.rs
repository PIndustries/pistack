// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use crate::error::Result;
use crate::tls::Pki;
use crate::types::CertUsage;

impl Client {
    /// Client certificate an agent or service presents for mTLS.
    pub fn issue_client_cert(&self, cn: &str) -> Result<Pki> {
        self.issue_pki(cn, Vec::new(), CertUsage::Client, None)
    }
}
