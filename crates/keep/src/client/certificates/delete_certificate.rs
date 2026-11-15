// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::Id;

use crate::client::Client;
use crate::error::Result;
use crate::quota::CERTIFICATES;
use crate::types::{Certificate, StoredSecret};

impl Client {
    /// Delete a certificate and its key secret.
    pub fn delete_certificate(&self, id: &Id) -> Result<Certificate> {
        let cert = self.store.collection::<Certificate>().delete(id)?;
        if let Some(k) = &cert.status.key_secret {
            match self.store.collection::<StoredSecret>().delete(k) {
                Ok(_) | Err(pistack_store::StoreError::NotFound(_)) => {}
                Err(e) => return Err(e.into()),
            }
        }
        self.note_usage(cert.meta.project.as_ref(), CERTIFICATES);
        Ok(cert)
    }
}
