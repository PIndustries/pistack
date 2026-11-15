// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::Id;

use crate::access_key::ROLE_ACCESS_KEY;
use crate::client::helpers::managed_role;
use crate::client::Client;
use crate::error::Result;
use crate::quota::SECRETS;
use crate::types::public_secret;
use crate::types::Secret;
use crate::types::StoredSecret;

impl Client {
    /// Delete a secret. Deleting an access key's secret revokes the key.
    pub fn delete_secret(&self, id: &Id) -> Result<Secret> {
        let s = self.store.collection::<Secret>().get(id)?;
        if managed_role(&s.meta.labels) == Some(ROLE_ACCESS_KEY) {
            self.revoke_access_key(id)?;
            return Ok(s);
        }
        let gone = self.store.collection::<StoredSecret>().delete(id)?;
        self.note_usage(gone.meta.project.as_ref(), SECRETS);
        Ok(public_secret(gone))
    }
}
