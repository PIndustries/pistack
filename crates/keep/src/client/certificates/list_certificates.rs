// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::Id;

use crate::client::Client;
use crate::error::Result;
use crate::types::Certificate;

impl Client {
    pub fn list_certificates(&self, project: Option<&Id>) -> Result<Vec<Certificate>> {
        Ok(self.store.collection::<Certificate>().list(project)?)
    }
}
