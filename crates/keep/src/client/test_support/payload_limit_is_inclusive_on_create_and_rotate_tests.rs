// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::client::client;
use crate::client::MAX_PAYLOAD;
use crate::error::KeepError;
use crate::types::*;

#[cfg(test)]
#[test]
fn payload_limit_is_inclusive_on_create_and_rotate() {
    let c = client();
    let at = vec![b'a'; MAX_PAYLOAD];
    let over = vec![b'a'; MAX_PAYLOAD + 1];
    let s = c
        .create_secret(Secret::new("big", None, SecretSpec::default()), &at)
        .unwrap();
    assert_eq!(c.get_secret_payload(&s.meta.id).unwrap().len(), MAX_PAYLOAD);
    assert!(matches!(
        c.create_secret(Secret::new("big2", None, SecretSpec::default()), &over),
        Err(KeepError::Invalid(_))
    ));
    assert!(matches!(
        c.rotate_secret(&s.meta.id, &over, None),
        Err(KeepError::Invalid(_))
    ));
    assert!(matches!(
        c.rotate_secret(&s.meta.id, b"", None),
        Err(KeepError::Invalid(_))
    ));
    c.rotate_secret(&s.meta.id, &at, None).unwrap();
}
