// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Helpers for S3 UploadPartCopy / CopyObject (`x-amz-copy-source`, range,
//! conditionals). Range parsing is for UploadPartCopy only; CopyObject
//! rejects `x-amz-copy-source-range`.

use crate::multipart::s3_err;
use crate::types::Object;
use chrono::{DateTime, Utc};
use pistack_types::ApiResult;

/// Parsed `x-amz-copy-source` value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopySource {
    pub bucket: String,
    pub key: String,
    /// Present when the client asked for a version; silo has no object
    /// versioning API for copies yet → callers return `NotImplemented`.
    pub version_id: Option<String>,
}

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

/// Evaluate UploadPartCopy conditional headers against the source object.
/// Unsupported/unrecognized values are treated as failed preconditions only
/// when they are present and do not match.
pub fn check_copy_source_conditions(
    obj: &Object,
    if_match: Option<&str>,
    if_none_match: Option<&str>,
    if_unmodified_since: Option<&str>,
    if_modified_since: Option<&str>,
) -> ApiResult<()> {
    let etag = object_etag(obj);
    if let Some(want) = if_match {
        if !etag_matches(&etag, want) {
            return Err(s3_err(
                "PreconditionFailed",
                "At least one of the pre-conditions you specified did not hold",
            ));
        }
    }
    if let Some(want) = if_none_match {
        if etag_matches(&etag, want) {
            return Err(s3_err(
                "PreconditionFailed",
                "At least one of the pre-conditions you specified did not hold",
            ));
        }
    }
    if let Some(raw) = if_unmodified_since {
        let since = parse_http_date(raw)?;
        if obj.meta.updated_at > since {
            return Err(s3_err(
                "PreconditionFailed",
                "At least one of the pre-conditions you specified did not hold",
            ));
        }
    }
    if let Some(raw) = if_modified_since {
        let since = parse_http_date(raw)?;
        if obj.meta.updated_at <= since {
            return Err(s3_err(
                "PreconditionFailed",
                "At least one of the pre-conditions you specified did not hold",
            ));
        }
    }
    Ok(())
}

fn object_etag(obj: &Object) -> String {
    if !obj.status.md5.is_empty() {
        obj.status.md5.clone()
    } else {
        obj.status.sha256.clone()
    }
}

fn etag_matches(etag: &str, want: &str) -> bool {
    let want = want.trim().trim_matches('"');
    let etag = etag.trim().trim_matches('"');
    // Weak validators (`W/"…"`) are uncommon for S3 copies; compare the strong form.
    let want = want.strip_prefix("W/").unwrap_or(want).trim_matches('"');
    etag.eq_ignore_ascii_case(want)
}

fn parse_http_date(raw: &str) -> ApiResult<DateTime<Utc>> {
    DateTime::parse_from_rfc2822(raw.trim())
        .or_else(|_| DateTime::parse_from_rfc3339(raw.trim()))
        .map(|d| d.with_timezone(&Utc))
        .map_err(|_| {
            s3_err(
                "InvalidArgument",
                "invalid HTTP date in copy-source conditional header",
            )
        })
}

fn percent_decode(s: &str) -> String {
    percent_encoding::percent_decode_str(s)
        .decode_utf8_lossy()
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_source_shapes() {
        let s = parse_copy_source("/bkt/a/b.txt").unwrap();
        assert_eq!(s.bucket, "bkt");
        assert_eq!(s.key, "a/b.txt");
        assert!(s.version_id.is_none());

        let s = parse_copy_source("bkt/key").unwrap();
        assert_eq!(s.bucket, "bkt");
        assert_eq!(s.key, "key");

        let s = parse_copy_source("/bkt/k%2Fpath?versionId=v1").unwrap();
        assert_eq!(s.key, "k/path");
        assert_eq!(s.version_id.as_deref(), Some("v1"));
    }

    #[test]
    fn parse_range_ok_and_bad() {
        assert_eq!(parse_copy_source_range("bytes=0-9", 100).unwrap(), (0, 10));
        assert_eq!(parse_copy_source_range("bytes=5-5", 10).unwrap(), (5, 1));
        assert!(parse_copy_source_range("bytes=0-99", 50).is_err());
        assert!(parse_copy_source_range("bytes=10-5", 50).is_err());
        assert!(parse_copy_source_range("0-9", 50).is_err());
    }
}
