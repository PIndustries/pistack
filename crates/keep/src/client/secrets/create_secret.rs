// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::Phase;

use crate::ca::RESERVED_LABEL_PREFIX;
use crate::client::helpers::{check_metadata, check_payload};
use crate::client::Client;
use crate::error::KeepError;
use crate::error::Result;
use crate::quota::{self, SECRETS};
use crate::types::{public_secret, Secret, SecretStatus, StoredSecret, StoredSecretStatus};

impl Client {
    /// Create a secret with its first payload version.
    pub fn create_secret(&self, mut secret: Secret, payload: &[u8]) -> Result<Secret> {
        check_payload(payload)?;
        check_metadata(&secret)?;
        if let Some(k) = secret
            .meta
            .labels
            .keys()
            .find(|k| k.starts_with(RESERVED_LABEL_PREFIX))
        {
            return Err(KeepError::Invalid(format!(
                "label {k} is reserved for keep ({RESERVED_LABEL_PREFIX}*)"
            )));
        }
        secret.status = SecretStatus {
            phase: Phase::Ready,
            version: 1,
            last_rotated: None,
        };
        let mut stored = StoredSecret {
            meta: secret.meta,
            spec: secret.spec,
            status: StoredSecretStatus {
                public: secret.status,
                sealed: None,
            },
        };
        let aad = stored.meta.id.to_string();
        stored.status.sealed = Some(self.root_key.seal(payload, aad.as_bytes())?);
        let project = stored.meta.project.clone();
        let created = {
            let _admission = quota::admission();
            self.admit(project.as_ref(), SECRETS)?;
            self.store.collection::<StoredSecret>().create(stored)?
        };
        self.note_usage(project.as_ref(), SECRETS);
        Ok(public_secret(created))
    }
}
