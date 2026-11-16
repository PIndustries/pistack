// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::Id;
use std::collections::BTreeMap;

use crate::client::helpers::check_meta_fields;
use crate::client::Client;
use crate::error::Result;
use crate::quota::CERTIFICATES;
use crate::types::{Certificate, CertificateSpec};

impl Client {
    /// [`issue_certificate`](Self::issue_certificate) with a description
    /// and labels, held to the same limits as a secret's. Nothing is issued
    /// when they, or the project's `certificates` quota, refuse.
    pub fn issue_certificate_with(
        &self,
        project: Option<Id>,
        name: &str,
        spec: CertificateSpec,
        description: String,
        labels: BTreeMap<String, String>,
    ) -> Result<Certificate> {
        check_meta_fields(&labels, &description)?;
        // Early refusal so a full project does not spend a serial; the
        // authoritative check is repeated under the lock in persist.
        self.admit(project.as_ref(), CERTIFICATES)?;
        let issued = self.ca.issue(&spec)?;
        self.persist_certificate(project, name, spec, issued, description, labels)
    }
}
