// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::collections::BTreeMap;

use crate::ca::{ROLE_AGENT, ROLE_LABEL, ROLE_SERVER};
use crate::client::helpers::sanitize;
use crate::client::Client;
use crate::error::Result;
use crate::tls::Pki;
use crate::types::{default_ttl_days, CertUsage, CertificateSpec};

impl Client {
    pub(super) fn issue_pki(
        &self,
        cn: &str,
        sans: Vec<String>,
        usage: CertUsage,
        csr_pem: Option<&str>,
    ) -> Result<Pki> {
        let spec = CertificateSpec {
            common_name: cn.to_owned(),
            sans,
            usage,
            ttl_days: default_ttl_days(),
            issuer: None,
        };
        let issued = match csr_pem {
            Some(csr) => self.ca.issue_for_request(&spec, csr)?,
            None => self.ca.issue(&spec)?,
        };
        let name = format!("{}-{}", sanitize(cn), issued.serial);
        let pki = Pki {
            cert_pem: issued.cert_pem.clone(),
            key_pem: issued.key_pem.clone(),
            ca_pem: self.ca.bundle_pem(),
        };
        let role = match usage {
            CertUsage::Client => ROLE_AGENT,
            _ => ROLE_SERVER,
        };
        let labels = BTreeMap::from([(ROLE_LABEL.to_owned(), role.to_owned())]);
        self.persist_certificate(None, &name, spec, issued, String::new(), labels)?;
        Ok(pki)
    }
}
