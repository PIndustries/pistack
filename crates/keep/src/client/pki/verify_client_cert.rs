// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use crate::error::Result;

impl Client {
    /// Verify a DER client certificate against the cloud CA; returns its CN.
    pub fn verify_client_cert(&self, der: &[u8]) -> Result<String> {
        self.ca.verify_client_cert(der)
    }
}
