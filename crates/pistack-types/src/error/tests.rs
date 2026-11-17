// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::api_error::ApiError;
use super::error_code::ErrorCode;
use axum::http::{header, StatusCode};
use axum::response::IntoResponse;
use chrono::TimeDelta as Duration;

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
