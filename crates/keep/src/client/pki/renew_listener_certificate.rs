// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::ca::pem_to_der;
use crate::ca::ROLE_SERVER;
use crate::client::helpers::managed_cert_role;
use crate::client::Client;
use crate::error::KeepError;
use crate::error::Result;
use crate::tls::Pki;
use crate::types::Certificate;

impl Client {
    /// Issue the successor of a server's listener certificate, given the
    /// certificate itself and a request for a new key.
    ///
    /// When this store has the certificate's record it is renewed like any
    /// other ([`renew_certificate`](Self::renew_certificate)). It may have
    /// none: a follower issues its first listener certificate from its own
    /// copy of the store, which does not replicate, and a restored store
    /// is older than the certificates made since. The certificate still
    /// carries this CA's signature, so one with the same name and SANs is
    /// issued in its place, and from then on there is a record.
    ///
    /// Records are matched by fingerprint, not by serial: a follower's own
    /// copy can have given out a serial this store gave to something else.
    /// A certificate this CA did not sign is refused, which is what keeps
    /// one an operator installed from being replaced.
    pub fn renew_listener_certificate(&self, cert_pem: &str, csr_pem: &str) -> Result<Pki> {
        let current = self.ca.read_listener_cert(&pem_to_der(cert_pem)?)?;
        let record = self
            .store
            .collection::<Certificate>()
            .list(None)?
            .into_iter()
            .find(|c| c.status.fingerprint_sha256 == current.fingerprint_sha256);
        match record {
            Some(rec) if managed_cert_role(&rec) == Some(ROLE_SERVER) => {
                self.renew_certificate(rec.status.serial, &current.common_name, csr_pem)
            }
            Some(_) => Err(KeepError::Denied(pistack_types::ApiError::forbidden(
                "only a server's listener certificate is renewed here",
            ))),
            None => {
                tracing::info!(
                    cn = %current.common_name,
                    "keep: no record of this listener certificate here; issuing one in its place"
                );
                self.issue_server_cert_for(&current.common_name, &current.sans, csr_pem)
            }
        }
    }
}
