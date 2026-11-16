// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::denied::denied;
use super::limited::limited;
use super::mk::mk;
use crate::error::KeepError;
use crate::quota::SECRETS;
use crate::types::*;
use pistack_types::{kind, Id};

#[cfg(test)]
#[test]
fn secrets_per_project_default_limit_is_exact() {
    let c = limited(3, 10);
    let p = Id::new(kind::PROJECT);
    let mut ids = Vec::new();
    for i in 0..3 {
        ids.push(
            c.create_secret(mk(&format!("s{i}"), &p), b"x")
                .unwrap()
                .meta
                .id,
        );
    }
    let e = denied(c.create_secret(mk("s3", &p), b"x"));
    assert_eq!(e.code, pistack_types::error::ErrorCode::QuotaExceeded);
    assert_eq!(e.details["resource"], SECRETS);
    assert_eq!(e.details["limit"], 3);
    assert_eq!(e.details["used"], 3);
    assert_eq!(
        pistack_types::ApiError::from(KeepError::Denied(e))
            .code
            .status(),
        axum::http::StatusCode::FORBIDDEN
    );
    // Other projects and cloud-scoped secrets are not affected.
    c.create_secret(mk("s0", &Id::new(kind::PROJECT)), b"x")
        .unwrap();
    c.create_secret(Secret::new("cloud", None, SecretSpec::default()), b"x")
        .unwrap();
    // A delete frees exactly one slot.
    c.delete_secret(&ids[0]).unwrap();
    c.create_secret(mk("s3", &p), b"x").unwrap();
    assert!(c.create_secret(mk("s4", &p), b"x").is_err());
}
