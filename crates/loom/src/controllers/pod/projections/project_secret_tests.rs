// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::controllers::pod::projections::*;
use base64::{engine::general_purpose::STANDARD, Engine};

#[test]
fn project_secret_splits_a_json_object_and_keeps_other_payloads() {
    let json = project_secret(br#"{"password":"s3cret-value"}"#);
    assert_eq!(
        json.data.get("password").map(String::as_str),
        Some("s3cret-value")
    );
    assert!(json.binary_data.is_empty());

    let text = project_secret(b"plain-token");
    assert_eq!(
        text.data.get("data").map(String::as_str),
        Some("plain-token")
    );

    let empty = project_secret(b"{}");
    assert_eq!(empty.data.get("data").map(String::as_str), Some("{}"));

    let odd = project_secret(br#"{"n":1}"#);
    assert_eq!(odd.data.get("data").map(String::as_str), Some(r#"{"n":1}"#));

    let raw = [0xff, 0xfe];
    let binary = project_secret(&raw);
    assert!(binary.data.is_empty());
    assert_eq!(
        binary.binary_data.get("data").map(String::as_str),
        Some(STANDARD.encode(raw).as_str())
    );
}
