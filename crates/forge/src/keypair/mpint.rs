// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::ApiResult;

use super::bad::bad;
use super::string_field::string_field;

/// A big-endian unsigned integer. A leading zero byte is the sign, which no
/// key part needs, and a high bit without it is a negative number.
pub(crate) fn mpint<'a>(blob: &'a [u8], at: &mut usize, what: &str) -> ApiResult<&'a [u8]> {
    let raw = string_field(blob, at, what)?;
    let (head, digits) = raw
        .split_first()
        .ok_or_else(|| bad(format!("{what} is empty")))?;
    if *head & 0x80 != 0 {
        // A negative number is not a key part. The sign is a leading zero,
        // which the caller measures against, so it stays in what is returned.
        return Err(bad(format!("{what} is not a positive integer")));
    }
    if digits.is_empty() {
        return Err(bad(format!("{what} is empty")));
    }
    Ok(raw)
}
