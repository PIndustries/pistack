// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::multipart::s3_err;
use pistack_types::ApiResult;

use super::copy_source::CopySource;
use super::percent_decode::percent_decode;

/// Parse `/bucket/key`, `bucket/key`, with optional URL-encoding and
/// `?versionId=…` query. The key may contain `/`.
pub fn parse_copy_source(raw: &str) -> ApiResult<CopySource> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err(s3_err(
            "InvalidArgument",
            "x-amz-copy-source must not be empty",
        ));
    }
    let (path_part, query) = raw.split_once('?').unwrap_or((raw, ""));
    let path = percent_decode(path_part);
    let path = path.trim_start_matches('/');
    let (bucket, key) = path
        .split_once('/')
        .ok_or_else(|| s3_err("InvalidArgument", "x-amz-copy-source must be /bucket/key"))?;
    if bucket.is_empty() || key.is_empty() {
        return Err(s3_err(
            "InvalidArgument",
            "x-amz-copy-source must be /bucket/key",
        ));
    }
    let mut version_id = None;
    for pair in query.split('&').filter(|p| !p.is_empty()) {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        let k = percent_decode(k);
        if k.eq_ignore_ascii_case("versionId") {
            let v = percent_decode(v);
            if !v.is_empty() {
                version_id = Some(v);
            }
        }
    }
    Ok(CopySource {
        bucket: bucket.to_owned(),
        key: key.to_owned(),
        version_id,
    })
}
