// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::ca::{pem_to_der, serial_from_der};
use crate::client::Client;
use crate::error::KeepError;
use crate::error::Result;
use crate::tls::Pki;
use crate::types::Certificate;

impl Client {
    /// Renew the certificate with serial `presented`, which the caller has
    /// just proved it holds by using it for TLS. The new one has the same
    /// name, SANs and usage and the key in `csr_pem`; its `key_pem` comes
    /// back empty. The old one is marked as renewed and stays valid until
    /// it expires, so the holder can keep it until the new one works.
    ///
    /// Refused when this cloud has no record of the certificate (it was
    /// deleted, which is how a host is removed for good), when it is
    /// revoked, or when `cn` is not the name it was issued for.
    pub fn renew_certificate(&self, presented: u64, cn: &str, csr_pem: &str) -> Result<Pki> {
        let old = self.managed_certificate(presented)?.ok_or_else(|| {
            KeepError::Unauthorized("this cloud has no record of that certificate".into())
        })?;
        let issuer = old
            .status
            .issuer
            .clone()
            .unwrap_or_else(|| self.ca.issuing_id().clone());
        if old.status.revoked || self.ca.crl(&issuer)?.iter().any(|r| r.serial == presented) {
            return Err(KeepError::Unauthorized("certificate is revoked".into()));
        }
        if old.spec.common_name != cn {
            return Err(KeepError::Denied(pistack_types::ApiError::forbidden(
                format!("certificate is for '{}', not '{cn}'", old.spec.common_name),
            )));
        }
        let pki = self.issue_pki(cn, old.spec.sans.clone(), old.spec.usage, Some(csr_pem))?;
        let serial = serial_from_der(&pem_to_der(&pki.cert_pem)?)?;
        self.store
            .collection::<Certificate>()
            .modify(&old.meta.id, |c| {
                c.status.renewed_by = Some(serial);
                Ok(())
            })?;
        tracing::info!(
            cn,
            old = presented,
            new = serial,
            "keep: certificate renewed"
        );
        Ok(pki)
    }
}
