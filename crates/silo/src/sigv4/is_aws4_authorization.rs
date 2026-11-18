// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// Scheme token is exactly `AWS4-HMAC-SHA256` followed by a space.
pub fn is_aws4_authorization(value: &str) -> bool {
    const PREFIX: &[u8] = b"AWS4-HMAC-SHA256 ";
    let v = value.as_bytes();
    v.len() >= PREFIX.len() && v[..PREFIX.len()].eq_ignore_ascii_case(PREFIX)
}
