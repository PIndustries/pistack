// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `ErrorCode`: the machine-readable kind of an `ApiError`.

use axum::http::StatusCode;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    BadRequest,
    Unauthorized,
    Forbidden,
    NotFound,
    Conflict,
    /// The resource is there but this method is not allowed on it. S3 answers
    /// a read of a delete marker this way.
    MethodNotAllowed,
    QuotaExceeded,
    /// Too many requests; the message says what to wait for.
    RateLimited,
    Unprocessable,
    Internal,
    Unavailable,
    NotImplemented,
}

impl std::fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = serde_json::to_value(self)
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_else(|| "internal".into());
        f.write_str(&s)
    }
}

impl ErrorCode {
    pub fn status(self) -> StatusCode {
        match self {
            ErrorCode::BadRequest => StatusCode::BAD_REQUEST,
            ErrorCode::Unauthorized => StatusCode::UNAUTHORIZED,
            ErrorCode::Forbidden => StatusCode::FORBIDDEN,
            ErrorCode::NotFound => StatusCode::NOT_FOUND,
            ErrorCode::Conflict => StatusCode::CONFLICT,
            ErrorCode::MethodNotAllowed => StatusCode::METHOD_NOT_ALLOWED,
            ErrorCode::QuotaExceeded => StatusCode::FORBIDDEN,
            ErrorCode::RateLimited => StatusCode::TOO_MANY_REQUESTS,
            ErrorCode::Unprocessable => StatusCode::UNPROCESSABLE_ENTITY,
            ErrorCode::Internal => StatusCode::INTERNAL_SERVER_ERROR,
            ErrorCode::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
            ErrorCode::NotImplemented => StatusCode::NOT_IMPLEMENTED,
        }
    }
}
