// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::cert_spec::cert_spec;
use super::denied::denied;
use super::limited::limited;
use super::mk::mk;
use crate::quota::CERTIFICATES;
use pistack_types::{kind, Id};

#[cfg(test)]
#[test]
fn certificates_per_project_limit_is_exact_and_keys_count_there() {
    // One secret allowed, two certificates: the certificates' key
    // secrets must not use up the secret slot.
    let c = limited(1, 2);
    let p = Id::new(kind::PROJECT);
    c.issue_certificate(Some(p.clone()), "a", cert_spec("a"))
        .unwrap();
    let b = c
        .issue_certificate(Some(p.clone()), "b", cert_spec("b"))
        .unwrap();
    let serial = c.store().get_meta::<u64>(crate::ca::META_SERIAL).unwrap();
    let e = denied(c.issue_certificate(Some(p.clone()), "c", cert_spec("c")));
    assert_eq!(e.details["resource"], CERTIFICATES);
    assert_eq!(e.details["limit"], 2);
    // Refused before signing: no serial spent, no key left behind.
    assert_eq!(
        c.store().get_meta::<u64>(crate::ca::META_SERIAL).unwrap(),
        serial
    );
    assert_eq!(c.list_secrets(Some(&p)).unwrap().len(), 2);
    c.create_secret(mk("only", &p), b"x").unwrap();
    assert!(c.create_secret(mk("more", &p), b"x").is_err());
    // Deleting a certificate frees a certificate slot.
    c.delete_certificate(&b.meta.id).unwrap();
    c.issue_certificate(Some(p.clone()), "c", cert_spec("c"))
        .unwrap();
    // Agent/server certificates (no project) are never limited.
    c.issue_client_cert("agent-1").unwrap();
}
