// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::client::client;
use crate::client::{
    MAX_CONTENT_TYPE, MAX_DESCRIPTION, MAX_LABELS, MAX_LABEL_KEY, MAX_LABEL_VALUE,
};
use crate::error::KeepError;
use crate::types::*;

#[cfg(test)]
#[test]
fn metadata_limits_apply_on_create_and_update() {
    let c = client();
    let mut s = Secret::new("m", None, SecretSpec::default());
    s.meta.description = "d".repeat(MAX_DESCRIPTION);
    for i in 0..MAX_LABELS {
        s.meta
            .labels
            .insert(format!("k{i}"), "v".repeat(MAX_LABEL_VALUE));
    }
    let s = c.create_secret(s, b"x").unwrap();

    let over = |f: fn(&mut Secret)| {
        let mut x = Secret::new("n", None, SecretSpec::default());
        f(&mut x);
        x
    };
    let cases: [fn(&mut Secret); 6] = [
        |x| x.meta.description = "d".repeat(MAX_DESCRIPTION + 1),
        |x| x.spec.content_type = "t".repeat(MAX_CONTENT_TYPE + 1),
        |x| {
            for i in 0..=MAX_LABELS {
                x.meta.labels.insert(format!("k{i}"), "v".into());
            }
        },
        |x| {
            x.meta
                .labels
                .insert("k".repeat(MAX_LABEL_KEY + 1), "v".into());
        },
        |x| {
            x.meta
                .labels
                .insert("k".into(), "v".repeat(MAX_LABEL_VALUE + 1));
        },
        |x| {
            x.meta.labels.insert(String::new(), "v".into());
        },
    ];
    for f in cases {
        // Create refuses it...
        assert!(matches!(
            c.create_secret(over(f), b"x"),
            Err(KeepError::Invalid(_))
        ));
        // ...and so does update, which used to skip every check.
        let mut cur = c.get_secret(&s.meta.id).unwrap();
        cur.meta.labels.clear();
        f(&mut cur);
        assert!(matches!(
            c.update_secret(cur, None),
            Err(KeepError::Invalid(_))
        ));
    }
    assert_eq!(
        c.get_secret(&s.meta.id).unwrap().meta.revision,
        s.meta.revision
    );
}
