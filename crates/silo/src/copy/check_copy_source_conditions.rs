// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::multipart::s3_err;
use crate::types::Object;
use pistack_types::ApiResult;

use super::etag_matches::etag_matches;
use super::object_etag::object_etag;
use super::parse_http_date::parse_http_date;

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
