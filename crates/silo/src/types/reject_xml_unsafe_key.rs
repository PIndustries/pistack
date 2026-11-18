// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::{ApiError, ApiResult};

/// Reject characters that make ListObjectsV2 (and similar) XML ill-formed
/// even with `encoding-type=url`. Applied only on S3 key-creating paths
/// (PutObject, CreateMultipartUpload, CopyObject destination); GET/HEAD/
/// DELETE and listing still accept existing objects whose keys contain them.
///
/// Rejected: U+0001..=U+001F (all C0, including tab/LF/CR), U+007F, U+FFFE,
/// U+FFFF. NUL is already rejected by [`validate_key`].
pub fn reject_xml_unsafe_key(key: &str) -> ApiResult<()> {
    for c in key.chars() {
        let u = c as u32;
        if (0x01..=0x1f).contains(&u) || u == 0x7f || u == 0xfffe || u == 0xffff {
            // Fixed message — never echo the key (may contain control bytes).
            return Err(ApiError::bad_request(
                "Object key contains XML-unsafe characters",
            ));
        }
    }
    Ok(())
}
