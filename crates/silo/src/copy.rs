// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Helpers for S3 UploadPartCopy / CopyObject (`x-amz-copy-source`, range,
//! conditionals), and for the `Range` header of GetObject. CopyObject rejects
//! `x-amz-copy-source-range`; GetObject reads only its own `Range`.

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

/// What a `Range` header asked for. `None` means the header is not a single
/// satisfiable byte range and the whole object is served, which is what S3
/// does with a multi-range or malformed header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GetRange {
    /// `bytes=a-b` or `bytes=a-`: from `first` to `last`, both included.
    From { first: u64, last: u64 },
    /// `bytes=-n`: the last `n` bytes.
    Suffix { length: u64 },
}

/// Parse a `Range` header for GetObject. Unlike
/// [`parse_copy_source_range`], a header this cannot read is not an error:
/// it answers `None` and the caller serves the whole object.
///
/// `bytes=a-b`, `bytes=a-` and `bytes=-n` are understood, with `n` of zero
/// or more. `a > b` is malformed, not unsatisfiable — S3 answers 200 with
/// the whole object for it.
pub fn parse_get_range(raw: &str) -> Option<GetRange> {
    let rest = raw.trim().strip_prefix("bytes=")?;
    if rest.contains(',') || !rest.contains('-') {
        return None;
    }
    let (first_s, last_s) = rest.split_once('-')?;
    let first_s = first_s.trim();
    let last_s = last_s.trim();
    if first_s.is_empty() {
        if last_s.is_empty() {
            return None;
        }
        return Some(GetRange::Suffix {
            length: last_s.parse::<u64>().ok()?,
        });
    }
    let first: u64 = first_s.parse().ok()?;
    if last_s.is_empty() {
        return Some(GetRange::From {
            first,
            last: u64::MAX,
        });
    }
    let last: u64 = last_s.parse().ok()?;
    if first > last {
        return None;
    }
    Some(GetRange::From { first, last })
}

/// The range clipped to an object of `size` bytes, as `(first, last)` with
/// both included, or `None` when the object cannot answer it and S3 answers
/// 416: a first byte at or past the end of a non-empty object, or a suffix of
/// zero bytes.
///
/// On an empty object nothing is unsatisfiable — every range asks for bytes
/// that are simply not there, and the answer is the empty object.
pub fn satisfiable(range: GetRange, size: u64) -> Option<(u64, u64)> {
    if size == 0 {
        return Some((0, 0));
    }
    match range {
        GetRange::From { first, last } => {
            if first >= size {
                return None;
            }
            Some((first, last.min(size - 1)))
        }
        GetRange::Suffix { length } => {
            if length == 0 {
                return None;
            }
            Some((size.saturating_sub(length), size - 1))
        }
    }
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

    #[test]
    fn get_range_reads_the_three_forms() {
        assert_eq!(
            parse_get_range("bytes=0-9"),
            Some(GetRange::From { first: 0, last: 9 })
        );
        assert_eq!(
            parse_get_range("bytes=40-"),
            Some(GetRange::From {
                first: 40,
                last: u64::MAX
            })
        );
        assert_eq!(
            parse_get_range("bytes=-20"),
            Some(GetRange::Suffix { length: 20 })
        );
        assert_eq!(
            parse_get_range("  bytes=1-2 "),
            Some(GetRange::From { first: 1, last: 2 })
        );
    }

    #[test]
    fn get_range_ignores_what_it_cannot_read() {
        for raw in [
            "bytes=0-10,20-30",
            "bytes=",
            "bytes=-",
            "bytes=abc-def",
            "bytes=10-5",
            "items=0-9",
            "bytes=0-9,10",
            "0-9",
        ] {
            assert_eq!(parse_get_range(raw), None, "{raw}");
        }
    }

    #[test]
    fn get_range_clips_to_the_object() {
        assert_eq!(
            satisfiable(GetRange::From { first: 0, last: 9 }, 100),
            Some((0, 9))
        );
        // Past the end of the object the last byte simply is the last byte.
        assert_eq!(
            satisfiable(
                GetRange::From {
                    first: 90,
                    last: u64::MAX
                },
                100
            ),
            Some((90, 99))
        );
        assert_eq!(
            satisfiable(
                GetRange::From {
                    first: 0,
                    last: u64::MAX
                },
                100
            ),
            Some((0, 99))
        );
        assert_eq!(satisfiable(GetRange::Suffix { length: 5 }, 3), Some((0, 2)));
        assert_eq!(satisfiable(GetRange::Suffix { length: 3 }, 3), Some((0, 2)));
    }

    #[test]
    fn get_range_refuses_what_the_object_cannot_answer() {
        assert_eq!(
            satisfiable(
                GetRange::From {
                    first: 100,
                    last: u64::MAX
                },
                100
            ),
            None
        );
        assert_eq!(satisfiable(GetRange::From { first: 5, last: 9 }, 3), None);
        // A suffix of zero bytes asks for nothing at all.
        assert_eq!(satisfiable(GetRange::Suffix { length: 0 }, 3), None);
    }

    #[test]
    fn every_range_on_an_empty_object_is_the_empty_object() {
        for range in [
            GetRange::From { first: 0, last: 9 },
            GetRange::From {
                first: 0,
                last: u64::MAX,
            },
            GetRange::From {
                first: 4,
                last: u64::MAX,
            },
            GetRange::Suffix { length: 0 },
            GetRange::Suffix { length: 9 },
        ] {
            assert_eq!(satisfiable(range, 0), Some((0, 0)), "{range:?}");
        }
    }
}
