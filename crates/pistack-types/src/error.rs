// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use axum::{
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use chrono::TimeDelta as Duration;
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

impl ApiError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            details: Default::default(),
        }
    }
    pub fn with_detail(mut self, key: &str, value: impl Into<serde_json::Value>) -> Self {
        self.details.insert(key.to_owned(), value.into());
        self
    }
    pub fn bad_request(m: impl Into<String>) -> Self {
        Self::new(ErrorCode::BadRequest, m)
    }
    pub fn unauthorized(m: impl Into<String>) -> Self {
        Self::new(ErrorCode::Unauthorized, m)
    }
    pub fn forbidden(m: impl Into<String>) -> Self {
        Self::new(ErrorCode::Forbidden, m)
    }
    pub fn not_found(m: impl Into<String>) -> Self {
        Self::new(ErrorCode::NotFound, m)
    }
    pub fn conflict(m: impl Into<String>) -> Self {
        Self::new(ErrorCode::Conflict, m)
    }
    pub fn quota(m: impl Into<String>) -> Self {
        Self::new(ErrorCode::QuotaExceeded, m)
    }
    /// Refused for rate: the reply carries `Retry-After: <secs>`. Part of a
    /// second is rounded up, and never to zero: the wait named must have
    /// passed before the caller tries again.
    pub fn rate_limited(m: impl Into<String>, retry_after: Duration) -> Self {
        let ms = retry_after.num_milliseconds();
        let secs = if ms > 0 { (ms - 1) / 1_000 + 1 } else { 1 };
        Self::new(ErrorCode::RateLimited, m).with_detail("retry_after_secs", secs)
    }
    pub fn unprocessable(m: impl Into<String>) -> Self {
        Self::new(ErrorCode::Unprocessable, m)
    }
    pub fn internal(m: impl Into<String>) -> Self {
        Self::new(ErrorCode::Internal, m)
    }
    pub fn unavailable(m: impl Into<String>) -> Self {
        Self::new(ErrorCode::Unavailable, m)
    }
    pub fn not_implemented(m: impl Into<String>) -> Self {
        Self::new(ErrorCode::NotImplemented, m)
    }
    pub fn status(&self) -> StatusCode {
        self.code.status()
    }
}

#[derive(Serialize)]
struct Envelope<'a> {
    error: &'a ApiError,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = self.status();
        let retry_after = self
            .details
            .get("retry_after_secs")
            .and_then(serde_json::Value::as_u64)
            .and_then(|s| axum::http::HeaderValue::from_str(&s.to_string()).ok());
        let mut response = (status, Json(Envelope { error: &self })).into_response();
        if let Some(v) = retry_after {
            response.headers_mut().insert(header::RETRY_AFTER, v);
        }
        response
    }
}

impl From<serde_json::Error> for ApiError {
    fn from(e: serde_json::Error) -> Self {
        ApiError::bad_request(format!("invalid json: {e}"))
    }
}

pub type ApiResult<T> = Result<T, ApiError>;

#[cfg(test)]
mod tests {
    use super::*;

    fn retry_after(e: ApiError) -> Option<String> {
        e.into_response()
            .headers()
            .get(header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned)
    }

    #[test]
    fn a_rate_limit_counts_the_seconds_to_wait() {
        let e = ApiError::rate_limited("try again later", Duration::milliseconds(899_400));
        assert_eq!(e.code, ErrorCode::RateLimited);
        assert_eq!(e.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(e.details["retry_after_secs"], 900);
        // A refusal with no time left is still a refusal that says wait.
        assert_eq!(
            ApiError::rate_limited("x", Duration::zero()).details["retry_after_secs"],
            1
        );
    }

    #[test]
    fn a_rate_limit_reply_carries_the_header() {
        let e = ApiError::rate_limited("too many failed sign-ins", Duration::seconds(120));
        assert_eq!(retry_after(e.clone()), Some("120".to_owned()));
        assert_eq!(e.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(e.details["retry_after_secs"], 120);
        assert_eq!(e.to_string(), "rate_limited: too many failed sign-ins");
    }

    #[test]
    fn another_error_has_no_retry_header() {
        let e = ApiError::unauthorized("invalid credentials");
        assert_eq!(retry_after(e), None);
    }
}
