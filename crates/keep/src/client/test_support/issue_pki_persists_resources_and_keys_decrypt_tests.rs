// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::client::client;

#[cfg(test)]
#[test]
fn issue_pki_persists_resources_and_keys_decrypt() {
    let c = client();
    let pki = c
        .issue_server_cert("gw.local", &["gw.local".into()])
        .unwrap();
    assert!(pki.ca_pem.matches("BEGIN CERTIFICATE").count() == 2);
    let certs = c.list_certificates(None).unwrap();
    assert_eq!(certs.len(), 1);
    assert_eq!(certs[0].status.cert_pem, pki.cert_pem);
    assert_eq!(c.certificate_key_pem(&certs[0]).unwrap(), pki.key_pem);

    let client_pki = c.issue_client_cert("agent-1").unwrap();
    let der = crate::ca::pem_to_der(&client_pki.cert_pem).unwrap();
    assert_eq!(c.verify_client_cert(&der).unwrap(), "agent-1");
    let cert = c
        .list_certificates(None)
        .unwrap()
        .into_iter()
        .find(|x| x.spec.common_name == "agent-1")
        .unwrap();
    let revoked = c.revoke_certificate(&cert.meta.id).unwrap();
    assert!(revoked.status.revoked);
    assert!(c.verify_client_cert(&der).is_err());
    // Delete removes the key secret too.
    let key = cert.status.key_secret.clone().unwrap();
    c.delete_certificate(&cert.meta.id).unwrap();
    assert!(c.get_secret(&key).is_err());
    assert_eq!(c.list_cas().unwrap().len(), 2);
}
