// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::client::client;
use super::serial_of::serial_of;
use crate::ca::ROLE_SERVER;
use crate::client::{managed_cert_role, Certificate};
use crate::error::KeepError;
use crate::types::*;

#[cfg(test)]
#[test]
fn a_listener_certificate_with_no_record_here_is_replaced_by_one_like_it() {
    let c = client();
    let sans = vec!["10.0.0.12".to_owned(), "pistack".to_owned()];
    let req = || crate::ca::new_key_request("pistack").unwrap().csr_pem;
    let count = || c.list_certificates(None).unwrap().len();

    // Recorded: renewed, and the old record says so.
    let known = c.issue_server_cert("pistack", &sans).unwrap();
    let renewed = c
        .renew_listener_certificate(&known.cert_pem, &req())
        .unwrap();
    let old = c.managed_certificate(serial_of(&known)).unwrap().unwrap();
    assert_eq!(old.status.renewed_by, Some(serial_of(&renewed)));

    // As a follower's first certificate is on the leader: signed by
    // this CA, no record. One like it is issued and recorded.
    let local = c.issue_server_cert("pistack", &sans).unwrap();
    let id = c
        .managed_certificate(serial_of(&local))
        .unwrap()
        .unwrap()
        .meta
        .id;
    c.delete_certificate(&id).unwrap();
    let before = count();
    let fresh = c
        .renew_listener_certificate(&local.cert_pem, &req())
        .unwrap();
    assert!(fresh.key_pem.is_empty());
    assert_eq!(count(), before + 1);
    let rec = c.managed_certificate(serial_of(&fresh)).unwrap().unwrap();
    assert_eq!(rec.spec.common_name, "pistack");
    assert_eq!(rec.spec.sans, sans);
    assert_eq!(rec.spec.usage, CertUsage::Server);
    assert_eq!(managed_cert_role(&rec), Some(ROLE_SERVER));

    // The same serial on another record is not the same certificate:
    // that record is left alone and the names come from the one shown.
    let other = c
        .issue_server_cert("pistack", &["10.0.0.99".to_owned()])
        .unwrap();
    let other_id = c
        .managed_certificate(serial_of(&other))
        .unwrap()
        .unwrap()
        .meta
        .id;
    let lost = c.issue_server_cert("pistack", &sans).unwrap();
    let lost_serial = serial_of(&lost);
    let lost_id = c.managed_certificate(lost_serial).unwrap().unwrap().meta.id;
    c.delete_certificate(&lost_id).unwrap();
    c.store()
        .collection::<Certificate>()
        .modify(&other_id, |r| {
            r.status.serial = lost_serial;
            Ok(())
        })
        .unwrap();
    let fresh = c
        .renew_listener_certificate(&lost.cert_pem, &req())
        .unwrap();
    let rec = c.managed_certificate(serial_of(&fresh)).unwrap().unwrap();
    assert_eq!(rec.spec.sans, sans);
    let untouched = c.get_certificate(&other_id).unwrap();
    assert_eq!(untouched.status.renewed_by, None);

    // Not this cloud's CA: an operator's own certificate stays.
    let foreign = client().issue_server_cert("pistack", &sans).unwrap();
    let before = count();
    let err = c
        .renew_listener_certificate(&foreign.cert_pem, &req())
        .unwrap_err();
    assert!(
        matches!(&err, KeepError::Unauthorized(m) if m.contains("not issued")),
        "{err}"
    );
    // An agent's certificate is not a listener's.
    let agent = c.issue_client_cert("n1").unwrap();
    let csr = crate::ca::new_key_request("n1").unwrap().csr_pem;
    assert!(c.renew_listener_certificate(&agent.cert_pem, &csr).is_err());
    assert_eq!(count(), before + 1);
}
