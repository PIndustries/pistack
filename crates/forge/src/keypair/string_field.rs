// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::ApiResult;

use super::bad::bad;

/// A length-prefixed field, the only thing a wire blob is made of.
pub(crate) fn string_field<'a>(blob: &'a [u8], at: &mut usize, what: &str) -> ApiResult<&'a [u8]> {
    if *at + 4 > blob.len() {
        return Err(bad(format!("{what} is missing")));
    }
    let len = u32::from_be_bytes([blob[*at], blob[*at + 1], blob[*at + 2], blob[*at + 3]]) as usize;
    if len > blob.len() - *at - 4 {
        return Err(bad(format!("{what} is truncated")));
    }
    *at += 4;
    let v = &blob[*at..*at + len];
    *at += len;
    Ok(v)
}
