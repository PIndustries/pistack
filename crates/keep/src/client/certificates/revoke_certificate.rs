// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::Id;

use crate::client::Client;
use crate::error::Result;
use crate::types::Certificate;

impl Client {
    /// Revoke on the issuing CA and mark the resource.
    pub fn revoke_certificate(&self, id: &Id) -> Result<Certificate> {
        let col = self.store.collection::<Certificate>();
        let cert = col.get(id)?;
        let issuer = cert
            .status
            .issuer
            .clone()
            .unwrap_or_else(|| self.ca.issuing_id().clone());
        self.ca.revoke(&issuer, cert.status.serial)?;
        Ok(col.modify(id, |c| {
            c.status.revoked = true;
            Ok(())
        })?)
    }
}
