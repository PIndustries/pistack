// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::{ApiError, ApiResult};

/// Reject empty keys, absolute paths, and any `.` / `..` component.
///
/// Object bytes are joined onto the bucket directory, so a key that climbs
/// out of that directory would write outside the data dir.
pub fn validate_key(key: &str) -> ApiResult<()> {
    if key.is_empty() {
        return Err(ApiError::bad_request("object key is empty"));
    }
    if key.contains('\0') {
        return Err(ApiError::bad_request("object key contains NUL"));
    }
    if key.contains('\\') {
        return Err(ApiError::bad_request("object key must not contain '\\'"));
    }
    if key.starts_with('/') {
        return Err(ApiError::bad_request("object key must not be absolute"));
    }
    let bytes = key.as_bytes();
    if bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic() {
        return Err(ApiError::bad_request("object key must not be absolute"));
    }
    if key == ".blobs" || key.starts_with(".blobs/") {
        return Err(ApiError::bad_request("object key is reserved"));
    }
    if key == ".uploads" || key.starts_with(".uploads/") {
        return Err(ApiError::bad_request("object key is reserved"));
    }
    for seg in key.split('/') {
        if seg.is_empty() || seg == "." || seg == ".." {
            return Err(ApiError::bad_request(
                "object key must be a relative path without '.' or '..'",
            ));
        }
    }
    Ok(())
}
