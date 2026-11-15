// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::Id;

use crate::client::Client;
use crate::error::Result;
use crate::types::Secret;

impl Client {
    pub fn list_secrets(&self, project: Option<&Id>) -> Result<Vec<Secret>> {
        Ok(self.store.collection::<Secret>().list(project)?)
    }
}
