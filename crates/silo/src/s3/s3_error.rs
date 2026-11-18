// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::multipart::s3_code_of;
use axum::{
    http::{header, StatusCode},
    response::{IntoResponse, Response},
};
use pistack_types::error::ErrorCode;
use pistack_types::ApiError;

use super::error_xml::error_xml;
use super::s3_method_not_allowed::s3_method_not_allowed;

pub(crate) fn s3_error(err: ApiError) -> Response {
    if err.code == ErrorCode::MethodNotAllowed {
        return s3_method_not_allowed(&err);
    }
    let (status, default_code) = match err.status() {
        StatusCode::NOT_FOUND => (StatusCode::NOT_FOUND, "NoSuchKey"),
        StatusCode::UNAUTHORIZED => (StatusCode::FORBIDDEN, "AccessDenied"),
        StatusCode::FORBIDDEN => (StatusCode::FORBIDDEN, "AccessDenied"),
        StatusCode::BAD_REQUEST => (StatusCode::BAD_REQUEST, "InvalidArgument"),
        StatusCode::CONFLICT => (StatusCode::CONFLICT, "Conflict"),
        other => (other, "InternalError"),
    };
    let code = if let Some(c) = s3_code_of(&err) {
        c
    } else if status == StatusCode::NOT_FOUND
        && (err.message.contains("bkt ") || err.message.contains("bucket"))
    {
        "NoSuchBucket"
    } else {
        default_code
    };
    (
        status,
        [(header::CONTENT_TYPE, "application/xml")],
        error_xml(code, &err.message),
    )
        .into_response()
}
