// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::chunked::TrailerChecksum;
use crate::multipart::s3_err;
use axum::http::HeaderMap;
use pistack_types::ApiResult;

use super::header_str::header_str;

pub(crate) fn trailer_from_headers(headers: &HeaderMap) -> ApiResult<TrailerChecksum> {
    let raw = header_str(headers, "x-amz-trailer").ok_or_else(|| {
        s3_err(
            "InvalidRequest",
            "missing x-amz-trailer for streaming trailer upload",
        )
    })?;
    // Clients may send one name, or a comma-separated list; we accept the first
    // checksum header name.
    let first = raw.split(',').next().unwrap_or(raw).trim();
    TrailerChecksum::parse(first)
}
