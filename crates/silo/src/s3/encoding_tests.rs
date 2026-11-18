// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::decode_continuation_token::decode_continuation_token;
use super::encode_continuation_token::encode_continuation_token;
use super::s3_url_encode::s3_url_encode;

#[test]
fn continuation_token_round_trip_and_rejects_garbage() {
    let tok = encode_continuation_token("a b/");
    assert!(!tok.contains('/'), "{tok}");
    assert!(!tok.contains('+'), "{tok}");
    assert_eq!(decode_continuation_token(&tok).unwrap(), "a b/");
    let ctrl = encode_continuation_token("a\u{1}");
    assert_eq!(decode_continuation_token(&ctrl).unwrap(), "a\u{1}");
    assert!(decode_continuation_token("!!!").is_err());
    assert!(decode_continuation_token("====").is_err());
}

#[test]
fn s3_url_encode_matches_botocore_quote_safe_slash() {
    assert_eq!(s3_url_encode("a b"), "a%20b");
    assert_eq!(s3_url_encode("a+b"), "a%2Bb");
    assert_eq!(s3_url_encode("a%b"), "a%25b");
    assert_eq!(s3_url_encode("a&b"), "a%26b");
    assert_eq!(s3_url_encode("café"), "caf%C3%A9");
    assert_eq!(s3_url_encode("a/b"), "a/b");
    assert_eq!(s3_url_encode("test_file(3).png"), "test_file%283%29.png");
    assert_eq!(s3_url_encode("x%2Fy"), "x%252Fy");
    assert_eq!(s3_url_encode("a b/"), "a%20b/");
}
