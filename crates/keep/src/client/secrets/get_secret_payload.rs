// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::Id;

use crate::access_key::ROLE_ACCESS_KEY;
use crate::ca::open_secret;
use crate::client::helpers::managed_role;
use crate::client::Client;
use crate::error::KeepError;
use crate::error::Result;
use crate::types::Secret;

impl Client {
    /// Decrypted payload of a secret. An access key's secret is refused:
    /// it is shown once at creation and otherwise only reaches
    /// [`lookup_access_key`](Self::lookup_access_key).
    pub fn get_secret_payload(&self, id: &Id) -> Result<Vec<u8>> {
        let s = self.store.collection::<Secret>().get(id)?;
        if managed_role(&s.meta.labels) == Some(ROLE_ACCESS_KEY) {
            return Err(KeepError::Conflict(
                "an access key's secret is shown only once, when it is created".into(),
            ));
        }
        open_secret(&self.store, &self.root_key, id)
    }
}
