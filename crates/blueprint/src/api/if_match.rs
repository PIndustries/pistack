// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Parsing the `If-Match` revision header.

use axum::http::header::IF_MATCH;
use axum::http::HeaderMap;
use pistack_types::{ApiError, ApiResult};

pub(super) fn if_match(headers: &HeaderMap) -> ApiResult<Option<u64>> {
    match headers.get(IF_MATCH) {
        None => Ok(None),
        Some(v) => {
            let s = v
                .to_str()
                .map_err(|_| ApiError::bad_request("If-Match is not ascii"))?;
            s.trim()
                .trim_matches('"')
                .parse::<u64>()
                .map(Some)
                .map_err(|_| ApiError::bad_request("If-Match must be a revision number"))
        }
    }
}
