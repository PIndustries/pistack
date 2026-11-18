// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use axum::{
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use pistack_types::ApiResult;

/// `200` with `Location: /<bucket>`, the CreateBucket success answer.
pub(crate) fn location(bucket: &str) -> ApiResult<Response> {
    let mut resp = StatusCode::OK.into_response();
    if let Ok(v) = HeaderValue::from_str(&format!("/{bucket}")) {
        resp.headers_mut().insert(header::LOCATION, v);
    }
    Ok(resp)
}
