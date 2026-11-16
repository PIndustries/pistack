// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::Id;

use crate::client::Client;
use crate::error::Result;
use crate::types::CertificateAuthority;

impl Client {
    pub fn get_ca(&self, id: &Id) -> Result<CertificateAuthority> {
        Ok(self.store.collection::<CertificateAuthority>().get(id)?)
    }
}
