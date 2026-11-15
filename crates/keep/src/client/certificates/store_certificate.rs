// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::{Id, Phase};
use std::collections::BTreeMap;

use crate::ca::{seal_pem_secret, Issued};
use crate::client::Client;
use crate::error::Result;
use crate::types::{Certificate, CertificateSpec, CertificateStatus, StoredSecret};

impl Client {
    pub(super) fn store_certificate(
        &self,
        project: Option<Id>,
        name: &str,
        spec: CertificateSpec,
        issued: Issued,
        description: String,
        labels: BTreeMap<String, String>,
    ) -> Result<Certificate> {
        // A certificate issued for a request has no key here to keep.
        let key_secret = if issued.key_pem.is_empty() {
            None
        } else {
            Some(seal_pem_secret(
                &self.store,
                &self.root_key,
                &format!("{name}-key"),
                project.clone(),
                &issued.key_pem,
            )?)
        };
        let mut cert = Certificate::new(name, project, spec);
        cert.meta.description = description;
        cert.meta.labels = labels;
        cert.status = CertificateStatus {
            phase: Phase::Ready,
            serial: issued.serial,
            not_before: Some(issued.not_before),
            not_after: Some(issued.not_after),
            fingerprint_sha256: issued.fingerprint_sha256,
            cert_pem: issued.cert_pem,
            issuer: Some(issued.issuer),
            key_secret: key_secret.clone(),
            revoked: false,
            renewed_by: None,
        };
        match self.store.collection::<Certificate>().create(cert) {
            Ok(c) => Ok(c),
            Err(e) => {
                // Do not leave an orphaned key behind.
                if let Some(k) = &key_secret {
                    let _ = self.store.collection::<StoredSecret>().delete(k);
                }
                Err(e.into())
            }
        }
    }
}
