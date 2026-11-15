// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::Id;

use crate::client::Client;
use crate::error::Result;
use crate::types::Certificate;

impl Client {
    pub fn get_certificate(&self, id: &Id) -> Result<Certificate> {
        Ok(self.store.collection::<Certificate>().get(id)?)
    }
}
