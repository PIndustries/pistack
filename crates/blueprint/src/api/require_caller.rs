// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The `require_caller` middleware: a request without a `Caller` is refused.

use axum::{
    extract::Request,
    middleware::Next,
    response::{IntoResponse, Response},
};
use pistack_types::{ApiError, Caller};

/// The gateway attaches a `Caller`. A request without one is refused.
pub(super) async fn require_caller(req: Request, next: Next) -> Response {
    if req.extensions().get::<Caller>().is_none() {
        return ApiError::unauthorized("no credentials").into_response();
    }
    next.run(req).await
}
