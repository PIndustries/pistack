// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use axum::{
    body::Body,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use pistack_types::ApiError;

/// S3 answers a refused method with an empty body, so the usual error document
/// is not sent. `x-amz-delete-marker` is added by the caller, which sees the
/// marker in the error.
pub(crate) fn s3_method_not_allowed(_err: &ApiError) -> Response {
    let mut resp = StatusCode::METHOD_NOT_ALLOWED.into_response();
    *resp.body_mut() = Body::empty();
    resp
}
