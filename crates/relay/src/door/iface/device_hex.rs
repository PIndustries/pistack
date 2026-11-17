// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Twelve lowercase hex digits pulled from an id.

use pistack_types::{ApiError, ApiResult};

/// Twelve lowercase hex digits. Split on the first `-`. When the left side
/// is all ASCII letters and the right side is non-empty, only the right
/// side is used.
pub(crate) fn device_hex(id: &str) -> ApiResult<String> {
    let source = match id.split_once('-') {
        Some((kind, rest)) if !rest.is_empty() && kind.chars().all(|c| c.is_ascii_alphabetic()) => {
            rest
        }
        _ => id,
    };
    let mut hex = String::with_capacity(12);
    for c in source.chars() {
        if c.is_ascii_hexdigit() {
            hex.push(c.to_ascii_lowercase());
            if hex.len() == 12 {
                return Ok(hex);
            }
        }
    }
    Err(ApiError::bad_request(format!(
        "{id} does not yield 12 hex digits"
    )))
}
