// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::Id;
use std::collections::BTreeMap;

use crate::ca::Issued;
use crate::client::Client;
use crate::error::Result;
use crate::quota::{self, CERTIFICATES};
use crate::types::{Certificate, CertificateSpec};

impl Client {
    pub(in crate::client) fn persist_certificate(
        &self,
        project: Option<Id>,
        name: &str,
        spec: CertificateSpec,
        issued: Issued,
        description: String,
        labels: BTreeMap<String, String>,
    ) -> Result<Certificate> {
        let created = {
            let _admission = quota::admission();
            self.admit(project.as_ref(), CERTIFICATES)?;
            self.store_certificate(project.clone(), name, spec, issued, description, labels)?
        };
        self.note_usage(project.as_ref(), CERTIFICATES);
        Ok(created)
    }
}
