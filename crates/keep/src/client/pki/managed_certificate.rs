// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::helpers::managed_cert_role;
use crate::client::Client;
use crate::error::Result;
use crate::types::Certificate;

impl Client {
    /// The cloud's own certificate with this serial: one issued for an
    /// agent or a server listener, not one a project asked for.
    pub fn managed_certificate(&self, serial: u64) -> Result<Option<Certificate>> {
        Ok(self
            .store
            .collection::<Certificate>()
            .list(None)?
            .into_iter()
            .find(|c| c.status.serial == serial && managed_cert_role(c).is_some()))
    }
}
