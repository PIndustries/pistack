// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::client::client;
use super::serial_of::serial_of;
use crate::ca::ROLE_SERVER;
use crate::client::managed_cert_role;
use crate::types::*;

#[cfg(test)]
#[test]
fn a_server_certificate_keeps_its_names_when_renewed() {
    let c = client();
    let sans = vec!["10.0.0.11".to_owned(), "pistack".to_owned()];
    let first = c.issue_server_cert("pistack", &sans).unwrap();
    let req = crate::ca::new_key_request("pistack").unwrap();
    let renewed = c
        .renew_certificate(serial_of(&first), "pistack", &req.csr_pem)
        .unwrap();
    let rec = c.managed_certificate(serial_of(&renewed)).unwrap().unwrap();
    assert_eq!(rec.spec.sans, sans);
    assert_eq!(rec.spec.usage, CertUsage::Server);
    assert_eq!(managed_cert_role(&rec), Some(ROLE_SERVER));
}
