// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use crate::error::Result;
use crate::tls::Pki;
use crate::types::CertUsage;

impl Client {
    /// Server certificate for a gateway or agent endpoint. `sans` are DNS
    /// names and IPs; the common name is not implied as a SAN.
    pub fn issue_server_cert(&self, cn: &str, sans: &[String]) -> Result<Pki> {
        self.issue_pki(cn, sans.to_vec(), CertUsage::Server, None)
    }
}
