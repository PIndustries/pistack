// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::reserved_labels::reserved_labels;
use crate::ca::RESERVED_LABEL_PREFIX;
use crate::ca::ROLE_LABEL;
use crate::client::helpers::check_metadata;
use crate::client::Client;
use crate::error::KeepError;
use crate::error::Result;
use crate::types::{public_secret, Secret, StoredSecret, StoredSecretStatus};

impl Client {
    /// Replace spec and metadata (not the payload). Honours `If-Match`.
    /// The same metadata limits as create apply, and keep's own labels
    /// (`keep.pistack.io/*`) cannot be added, changed or removed.
    pub fn update_secret(&self, secret: Secret, expected_revision: Option<u64>) -> Result<Secret> {
        check_metadata(&secret)?;
        let col = self.store.collection::<StoredSecret>();
        let current = col.get(&secret.meta.id)?;
        if reserved_labels(&secret.meta.labels) != reserved_labels(&current.meta.labels) {
            return Err(KeepError::Conflict(format!(
                "labels {RESERVED_LABEL_PREFIX}* are managed by keep and cannot be changed \
                 (including {ROLE_LABEL})"
            )));
        }
        let stored = StoredSecret {
            meta: secret.meta,
            spec: secret.spec,
            status: StoredSecretStatus {
                public: current.status.public,
                sealed: current.status.sealed,
            },
        };
        Ok(public_secret(col.update(stored, expected_revision)?))
    }
}
