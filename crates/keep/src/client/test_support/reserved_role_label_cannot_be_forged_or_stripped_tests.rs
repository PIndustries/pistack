// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::client::client;
use crate::ca::{ROLE_LABEL, ROLE_PRIVATE_KEY};
use crate::client::CertificateSpec;
use crate::error::KeepError;
use crate::types::*;
use pistack_types::{kind, Id};

#[cfg(test)]
#[test]
fn reserved_role_label_cannot_be_forged_or_stripped() {
    let c = client();
    let proj = Id::new(kind::PROJECT);
    // Callers may not create a secret that claims to be a cert key.
    let mut forged = Secret::new("fake-key", Some(proj.clone()), SecretSpec::default());
    forged
        .meta
        .labels
        .insert(ROLE_LABEL.into(), ROLE_PRIVATE_KEY.into());
    assert!(matches!(
        c.create_secret(forged, b"x"),
        Err(KeepError::Invalid(_))
    ));

    let cert = c
        .issue_certificate(
            Some(proj.clone()),
            "web",
            CertificateSpec {
                common_name: "web".into(),
                ..Default::default()
            },
        )
        .unwrap();
    let key_id = cert.status.key_secret.clone().unwrap();
    let key_secret = c.get_secret(&key_id).unwrap();
    // Stripping the label (which would let the key be deleted out from
    // under its certificate) is refused...
    let mut stripped = key_secret.clone();
    stripped.meta.labels.remove(ROLE_LABEL);
    assert!(matches!(
        c.update_secret(stripped, None),
        Err(KeepError::Conflict(_))
    ));
    // ...as is adding it to an ordinary secret...
    let plain = c
        .create_secret(Secret::new("p", Some(proj), SecretSpec::default()), b"x")
        .unwrap();
    let mut tagged = plain.clone();
    tagged
        .meta
        .labels
        .insert(ROLE_LABEL.into(), ROLE_PRIVATE_KEY.into());
    assert!(matches!(
        c.update_secret(tagged, None),
        Err(KeepError::Conflict(_))
    ));
    // ...and rotating the key away from its certificate.
    assert!(matches!(
        c.rotate_secret(&key_id, b"not a key", None),
        Err(KeepError::Conflict(_))
    ));
    assert!(c
        .certificate_key_pem(&cert)
        .unwrap()
        .contains("PRIVATE KEY"));
    // Editing other metadata on the key secret still works.
    let mut desc = key_secret;
    desc.meta.description = "web key".into();
    assert_eq!(
        c.update_secret(desc, None).unwrap().meta.description,
        "web key"
    );
}
