// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::multipart::s3_err;
use pistack_types::ApiResult;

/// Parse `bytes=first-last` (inclusive). Returns `(start, length)`.
pub fn parse_copy_source_range(raw: &str, object_size: u64) -> ApiResult<(u64, u64)> {
    let rest = raw.trim().strip_prefix("bytes=").ok_or_else(|| {
        s3_err(
            "InvalidArgument",
            "x-amz-copy-source-range must be bytes=first-last",
        )
    })?;
    let (first_s, last_s) = rest.split_once('-').ok_or_else(|| {
        s3_err(
            "InvalidArgument",
            "x-amz-copy-source-range must be bytes=first-last",
        )
    })?;
    if first_s.is_empty() || last_s.is_empty() {
        return Err(s3_err(
            "InvalidArgument",
            "x-amz-copy-source-range must be bytes=first-last",
        ));
    }
    let first: u64 = first_s.parse().map_err(|_| {
        s3_err(
            "InvalidArgument",
            "x-amz-copy-source-range first must be a number",
        )
    })?;
    let last: u64 = last_s.parse().map_err(|_| {
        s3_err(
            "InvalidArgument",
            "x-amz-copy-source-range last must be a number",
        )
    })?;
    if first > last {
        return Err(s3_err(
            "InvalidArgument",
            "x-amz-copy-source-range first must be <= last",
        ));
    }
    if object_size == 0 || last >= object_size {
        return Err(s3_err(
            "InvalidRange",
            "The requested range is not satisfiable",
        ));
    }
    Ok((first, last - first + 1))
}
