// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};

/// The single error shape returned by every PiStack API.
///
/// Wire form:
/// ```json
/// { "error": { "code": "not_found", "message": "machine m-01HX... not found", "details": {} } }
/// ```
#[derive(Debug, thiserror::Error, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[error("{code}: {message}")]
pub struct ApiError {
    pub code: ErrorCode,
    pub message: String,
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub details: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    BadRequest,
    Unauthorized,
    Forbidden,
    NotFound,
    Conflict,
    QuotaExceeded,
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
            ErrorCode::QuotaExceeded => StatusCode::FORBIDDEN,
            ErrorCode::Unprocessable => StatusCode::UNPROCESSABLE_ENTITY,
            ErrorCode::Internal => StatusCode::INTERNAL_SERVER_ERROR,
            ErrorCode::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
            ErrorCode::NotImplemented => StatusCode::NOT_IMPLEMENTED,
        }
    }
}

impl ApiError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self { code, message: message.into(), details: Default::default() }
    }
    pub fn with_detail(mut self, key: &str, value: impl Into<serde_json::Value>) -> Self {
        self.details.insert(key.to_owned(), value.into());
        self
    }
    pub fn bad_request(m: impl Into<String>) -> Self { Self::new(ErrorCode::BadRequest, m) }
    pub fn unauthorized(m: impl Into<String>) -> Self { Self::new(ErrorCode::Unauthorized, m) }
    pub fn forbidden(m: impl Into<String>) -> Self { Self::new(ErrorCode::Forbidden, m) }
    pub fn not_found(m: impl Into<String>) -> Self { Self::new(ErrorCode::NotFound, m) }
    pub fn conflict(m: impl Into<String>) -> Self { Self::new(ErrorCode::Conflict, m) }
    pub fn quota(m: impl Into<String>) -> Self { Self::new(ErrorCode::QuotaExceeded, m) }
    pub fn unprocessable(m: impl Into<String>) -> Self { Self::new(ErrorCode::Unprocessable, m) }
    pub fn internal(m: impl Into<String>) -> Self { Self::new(ErrorCode::Internal, m) }
    pub fn unavailable(m: impl Into<String>) -> Self { Self::new(ErrorCode::Unavailable, m) }
    pub fn not_implemented(m: impl Into<String>) -> Self { Self::new(ErrorCode::NotImplemented, m) }
    pub fn status(&self) -> StatusCode { self.code.status() }
}

#[derive(Serialize)]
struct Envelope<'a> {
    error: &'a ApiError,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = self.status();
        (status, Json(Envelope { error: &self })).into_response()
    }
}

impl From<serde_json::Error> for ApiError {
    fn from(e: serde_json::Error) -> Self {
        ApiError::bad_request(format!("invalid json: {e}"))
    }
}

pub type ApiResult<T> = Result<T, ApiError>;
