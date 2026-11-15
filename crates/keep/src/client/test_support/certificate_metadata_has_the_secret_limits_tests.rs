// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::cert_spec::cert_spec;
use super::client::client;
use crate::client::{MAX_DESCRIPTION, MAX_LABELS, MAX_LABEL_VALUE};
use crate::error::KeepError;
use pistack_types::{kind, Id};
use std::collections::BTreeMap;

#[cfg(test)]
#[test]
fn certificate_metadata_has_the_secret_limits() {
    let c = client();
    let p = Some(Id::new(kind::PROJECT));
    let serial = c.store().get_meta::<u64>(crate::ca::META_SERIAL).unwrap();
    let too_long = "d".repeat(MAX_DESCRIPTION + 1);
    assert!(matches!(
        c.issue_certificate_with(p.clone(), "x", cert_spec("x"), too_long, BTreeMap::new()),
        Err(KeepError::Invalid(_))
    ));
    let mut many = BTreeMap::new();
    for i in 0..=MAX_LABELS {
        many.insert(format!("k{i}"), "v".to_owned());
    }
    assert!(matches!(
        c.issue_certificate_with(p.clone(), "x", cert_spec("x"), String::new(), many),
        Err(KeepError::Invalid(_))
    ));
    let mut big = BTreeMap::new();
    big.insert("k".to_owned(), "v".repeat(MAX_LABEL_VALUE + 1));
    assert!(matches!(
        c.issue_certificate_with(p.clone(), "x", cert_spec("x"), String::new(), big),
        Err(KeepError::Invalid(_))
    ));
    // Nothing was issued or stored.
    assert_eq!(
        c.store().get_meta::<u64>(crate::ca::META_SERIAL).unwrap(),
        serial
    );
    assert!(c.list_certificates(p.as_ref()).unwrap().is_empty());

    // At the limits it is accepted and stored in one write.
    let mut labels = BTreeMap::new();
    for i in 0..MAX_LABELS {
        labels.insert(format!("k{i}"), "v".repeat(MAX_LABEL_VALUE));
    }
    let cert = c
        .issue_certificate_with(
            p.clone(),
            "x",
            cert_spec("x"),
            "d".repeat(MAX_DESCRIPTION),
            labels,
        )
        .unwrap();
    assert_eq!(cert.meta.labels.len(), MAX_LABELS);
    assert_eq!(cert.meta.revision, 1);
}
