// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::client::client;
use super::serial_of::serial_of;
use crate::client::CertificateSpec;
use crate::error::KeepError;
use crate::types::*;
use pistack_types::Id;

#[cfg(test)]
#[test]
fn a_revoked_or_deleted_certificate_is_not_renewed() {
    let c = client();
    let req = || crate::ca::new_key_request("n1").unwrap().csr_pem;
    let count = || c.list_certificates(None).unwrap().len();

    let revoked = serial_of(&c.issue_client_cert("n1").unwrap());
    let id = c.managed_certificate(revoked).unwrap().unwrap().meta.id;
    c.revoke_certificate(&id).unwrap();
    let before = count();
    let err = c.renew_certificate(revoked, "n1", &req()).unwrap_err();
    assert!(
        matches!(&err, KeepError::Unauthorized(m) if m.contains("revoked")),
        "{err}"
    );

    // Revoked on the CA only, the record not marked.
    let on_ca = serial_of(&c.issue_client_cert("n1").unwrap());
    c.ca().revoke(c.ca().issuing_id(), on_ca).unwrap();
    assert!(matches!(
        c.renew_certificate(on_ca, "n1", &req()),
        Err(KeepError::Unauthorized(_))
    ));

    // The record deleted: the host was removed.
    let gone = serial_of(&c.issue_client_cert("n1").unwrap());
    let id = c.managed_certificate(gone).unwrap().unwrap().meta.id;
    c.delete_certificate(&id).unwrap();
    assert!(matches!(
        c.renew_certificate(gone, "n1", &req()),
        Err(KeepError::Unauthorized(_))
    ));
    // A serial this cloud never issued.
    assert!(c.renew_certificate(9999, "n1", &req()).is_err());
    // Nothing was issued by any of the refusals.
    assert_eq!(count(), before + 1);

    // A certificate a project asked for is not the cloud's to renew.
    let project = Id::new(pistack_types::kind::PROJECT);
    let theirs = c
        .issue_certificate(
            Some(project),
            "web",
            CertificateSpec {
                common_name: "n1".into(),
                usage: CertUsage::Client,
                ..Default::default()
            },
        )
        .unwrap();
    assert!(c
        .renew_certificate(theirs.status.serial, "n1", &req())
        .is_err());
}
