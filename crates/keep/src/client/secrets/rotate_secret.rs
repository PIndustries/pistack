// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use chrono::Utc;
use pistack_types::Id;
use pistack_types::Phase;

use crate::access_key::ROLE_ACCESS_KEY;
use crate::ca::ROLE_PRIVATE_KEY;
use crate::client::helpers::{check_payload, managed_role};
use crate::client::Client;
use crate::error::KeepError;
use crate::error::Result;
use crate::types::{public_secret, Secret, StoredSecret};

impl Client {
    /// Rotate: new payload, version + 1.
    pub fn rotate_secret(
        &self,
        id: &Id,
        payload: &[u8],
        expected_revision: Option<u64>,
    ) -> Result<Secret> {
        check_payload(payload)?;
        let col = self.store.collection::<StoredSecret>();
        let mut cur = col.get(id)?;
        match managed_role(&cur.meta.labels) {
            Some(ROLE_PRIVATE_KEY) => {
                return Err(KeepError::Conflict(
                    "this secret holds a certificate's key; it cannot be rotated on its own".into(),
                ))
            }
            Some(ROLE_ACCESS_KEY) => {
                return Err(KeepError::Conflict(
                    "an access key cannot be rotated; create a new one and revoke this one".into(),
                ))
            }
            Some(_) => {
                return Err(KeepError::Conflict(
                    "this secret is managed by keep and cannot be rotated".into(),
                ))
            }
            None => {}
        }
        if let Some(exp) = expected_revision {
            if exp != cur.meta.revision {
                return Err(pistack_store::StoreError::Revision {
                    expected: exp,
                    found: cur.meta.revision,
                }
                .into());
            }
        }
        let aad = id.to_string();
        cur.status.sealed = Some(self.root_key.seal(payload, aad.as_bytes())?);
        cur.status.public.version += 1;
        cur.status.public.last_rotated = Some(Utc::now());
        cur.status.public.phase = Phase::Ready;
        let rev = cur.meta.revision;
        Ok(public_secret(col.update(cur, Some(rev))?))
    }
}
