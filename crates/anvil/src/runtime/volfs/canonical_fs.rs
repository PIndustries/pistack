// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::{ApiError, ApiResult};

pub(in crate::runtime::volfs) fn canonical_fs(raw: &str) -> ApiResult<String> {
    let trimmed = raw.trim();
    let fs = if trimmed.is_empty() { "ext4" } else { trimmed };
    let fs = fs.to_ascii_lowercase();
    let bytes = fs.as_bytes();
    let ok = (2..=8).contains(&fs.len())
        && bytes[0].is_ascii_lowercase()
        && bytes
            .iter()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit());
    if ok {
        Ok(fs)
    } else {
        Err(ApiError::bad_request(format!(
            "filesystem type {raw} is not supported"
        )))
    }
}
