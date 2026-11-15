// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use crate::error::Result;
use crate::types::CertificateAuthority;

impl Client {
    pub fn list_cas(&self) -> Result<Vec<CertificateAuthority>> {
        Ok(self.store.collection::<CertificateAuthority>().list(None)?)
    }
}
