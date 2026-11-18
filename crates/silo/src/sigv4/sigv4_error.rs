// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::multipart::s3_code_of;
use axum::{
    http::{header, StatusCode},
    response::{IntoResponse, Response},
};
use pistack_types::ApiError;

use super::xml_escape::xml_escape;

pub(crate) fn sigv4_error(err: ApiError) -> Response {
    let code = s3_code_of(&err).unwrap_or("AccessDenied");
    let status = match code {
        "NotImplemented" => StatusCode::NOT_IMPLEMENTED,
        "AuthorizationHeaderMalformed"
        | "InvalidRequest"
        | "InvalidArgument"
        | "IncompleteBody"
        | "BadDigest"
        | "InvalidDigest"
        | "XAmzContentSHA256Mismatch"
        | "XAmzContentChecksumMismatch" => StatusCode::BAD_REQUEST,
        "InvalidAccessKeyId"
        | "SignatureDoesNotMatch"
        | "AccessDenied"
        | "RequestTimeTooSkewed" => StatusCode::FORBIDDEN,
        _ => err.status(),
    };
    let xml = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><Error><Code>{}</Code><Message>{}</Message></Error>",
        xml_escape(code),
        xml_escape(&err.message)
    );
    (status, [(header::CONTENT_TYPE, "application/xml")], xml).into_response()
}
