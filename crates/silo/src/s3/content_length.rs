// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use axum::http::{header, HeaderMap};
use pistack_types::{ApiError, ApiResult};

pub(crate) fn content_length(headers: &HeaderMap) -> ApiResult<Option<u64>> {
    match headers.get(header::CONTENT_LENGTH) {
        None => Ok(None),
        Some(v) => {
            let s = v
                .to_str()
                .map_err(|_| ApiError::bad_request("invalid Content-Length"))?;
            let n: u64 = s
                .parse()
                .map_err(|_| ApiError::bad_request("invalid Content-Length"))?;
            Ok(Some(n))
        }
    }
}
