// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::client::client;
use super::serial_of::serial_of;
use crate::ca::{pem_to_der, ROLE_AGENT};
use crate::client::{agent_cert_expiry, managed_cert_role};
use crate::error::KeepError;
use crate::types::*;

#[cfg(test)]
#[test]
fn a_certificate_is_renewed_for_a_new_key_and_the_old_one_is_marked() {
    let c = client();
    let first = c.issue_client_cert("n1").unwrap();
    let old = serial_of(&first);
    assert_eq!(
        managed_cert_role(&c.managed_certificate(old).unwrap().unwrap()),
        Some(ROLE_AGENT)
    );

    let req = crate::ca::new_key_request("n1").unwrap();
    let renewed = c.renew_certificate(old, "n1", &req.csr_pem).unwrap();
    // The key is the requester's: none comes back and none is stored.
    assert!(renewed.key_pem.is_empty());
    let new = serial_of(&renewed);
    assert_ne!(new, old);
    let rec = c.managed_certificate(new).unwrap().unwrap();
    assert!(rec.status.key_secret.is_none());
    assert_eq!(rec.spec.common_name, "n1");
    assert_eq!(rec.spec.usage, CertUsage::Client);
    assert_eq!(managed_cert_role(&rec), Some(ROLE_AGENT));
    // What the host list shows is the new one's last day.
    let all = c.list_certificates(None).unwrap();
    assert_eq!(agent_cert_expiry(&all, "n1"), rec.status.not_after);
    assert_eq!(agent_cert_expiry(&all, "n9"), None);
    // Both verify: the old one is kept until the new one works.
    assert_eq!(
        c.managed_certificate(old)
            .unwrap()
            .unwrap()
            .status
            .renewed_by,
        Some(new)
    );
    for pki in [&first, &renewed] {
        let der = pem_to_der(&pki.cert_pem).unwrap();
        assert_eq!(c.verify_client_cert(&der).unwrap(), "n1");
    }

    // Another node's name, with this node's certificate.
    let other = crate::ca::new_key_request("n2").unwrap();
    let err =
        pistack_types::ApiError::from(c.renew_certificate(new, "n2", &other.csr_pem).unwrap_err());
    assert_eq!(err.code, pistack_types::error::ErrorCode::Forbidden);
    // A request for another name than the one asked for.
    assert!(matches!(
        c.renew_certificate(new, "n1", &other.csr_pem),
        Err(KeepError::Invalid(_))
    ));
}
