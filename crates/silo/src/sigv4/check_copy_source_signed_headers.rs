// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::multipart::s3_err;
use axum::http::HeaderMap;
use pistack_types::ApiResult;

use super::copy_source_dependent_headers::COPY_SOURCE_DEPENDENT_HEADERS;
use super::header_str::header_str;
use super::parsed_auth::ParsedAuth;

/// Require copy-source (and present dependents, including
/// `x-amz-metadata-directive`) in SignedHeaders so a captured signature
/// cannot be replayed with a swapped source or directive.
///
/// When the directive is `REPLACE`, any present `Content-Type` must also be
/// signed — that is the only request header silo applies on REPLACE today
/// (no `x-amz-meta-*` / Content-Disposition / Encoding / Cache-Control).
pub(crate) fn check_copy_source_signed_headers(
    parsed: &ParsedAuth,
    headers: &HeaderMap,
) -> ApiResult<()> {
    if header_str(headers, "x-amz-copy-source").is_none() {
        return Ok(());
    }
    let has = |name: &str| {
        parsed
            .signed_headers
            .iter()
            .any(|h| h.eq_ignore_ascii_case(name))
    };
    if !has("x-amz-copy-source") {
        return Err(s3_err(
            "AccessDenied",
            "x-amz-copy-source must be in SignedHeaders",
        ));
    }
    for name in COPY_SOURCE_DEPENDENT_HEADERS {
        if header_str(headers, name).is_some() && !has(name) {
            return Err(s3_err(
                "AccessDenied",
                format!("{name} must be in SignedHeaders when present"),
            ));
        }
    }
    let directive = header_str(headers, "x-amz-metadata-directive").map(|v| v.to_ascii_uppercase());
    if directive.as_deref() == Some("REPLACE")
        && header_str(headers, "content-type").is_some()
        && !has("content-type")
    {
        return Err(s3_err(
            "AccessDenied",
            "content-type must be in SignedHeaders when x-amz-metadata-directive is REPLACE",
        ));
    }
    Ok(())
}
