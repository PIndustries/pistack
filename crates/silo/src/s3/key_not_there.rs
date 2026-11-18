// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::multipart::s3_code_of;
use axum::http::StatusCode;
use pistack_types::ApiError;

/// The key is not there, so there is nothing to delete. A bucket that is not
/// there is a different thing: the caller's whole request is wrong.
pub(crate) fn key_not_there(err: &ApiError) -> bool {
    s3_code_of(err) == Some("NoSuchKey")
        || (s3_code_of(err).is_none()
            && err.status() == StatusCode::NOT_FOUND
            && !err.message.contains("bucket"))
}
