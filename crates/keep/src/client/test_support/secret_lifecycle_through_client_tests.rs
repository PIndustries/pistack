// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::client::client;
use crate::types::*;
use pistack_types::{kind, Id};

#[cfg(test)]
#[test]
fn secret_lifecycle_through_client() {
    let c = client();
    let proj = Id::new(kind::PROJECT);
    let s = c
        .create_secret(
            Secret::new("db", Some(proj.clone()), SecretSpec::default()),
            b"pw1",
        )
        .unwrap();
    assert_eq!(s.status.version, 1);
    assert_eq!(c.get_secret_payload(&s.meta.id).unwrap(), b"pw1");
    let s2 = c
        .rotate_secret(&s.meta.id, b"pw2", Some(s.meta.revision))
        .unwrap();
    assert_eq!(s2.status.version, 2);
    assert!(s2.status.last_rotated.is_some());
    assert_eq!(c.get_secret_payload(&s.meta.id).unwrap(), b"pw2");
    // Stale revision.
    assert!(c.rotate_secret(&s.meta.id, b"pw3", Some(1)).is_err());
    // Metadata update keeps the payload.
    let mut m = c.get_secret(&s.meta.id).unwrap();
    m.meta.description = "prod db".into();
    let m = c.update_secret(m, None).unwrap();
    assert_eq!(m.status.version, 2);
    assert_eq!(c.get_secret_payload(&s.meta.id).unwrap(), b"pw2");
    assert!(c
        .create_secret(Secret::new("e", None, SecretSpec::default()), b"")
        .is_err());
    c.delete_secret(&s.meta.id).unwrap();
    assert!(c.get_secret(&s.meta.id).is_err());
}
