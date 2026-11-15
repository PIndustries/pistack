// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;

impl Client {
    /// Issuing CA then root: what every client should trust.
    pub fn ca_bundle_pem(&self) -> String {
        self.ca.bundle_pem()
    }
}
