// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::Id;
use std::collections::BTreeMap;

use crate::client::Client;
use crate::error::Result;
use crate::types::{Certificate, CertificateSpec};

impl Client {
    /// Issue and persist a certificate; its key becomes a Secret named
    /// `<name>-key` in the same project.
    pub fn issue_certificate(
        &self,
        project: Option<Id>,
        name: &str,
        spec: CertificateSpec,
    ) -> Result<Certificate> {
        self.issue_certificate_with(project, name, spec, String::new(), BTreeMap::new())
    }
}
