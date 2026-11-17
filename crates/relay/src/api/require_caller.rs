// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The gate in front of every route: the gateway's `Caller` must be on the
//! request.

use axum::extract::Request;
use axum::middleware::Next;
use axum::response::IntoResponse;
use axum::response::Response;
use pistack_types::{ApiError, Caller};

/// The gateway attaches a `Caller`. A request without one is refused.
pub(super) async fn require_caller(req: Request, next: Next) -> Response {
    if req.extensions().get::<Caller>().is_none() {
        return ApiError::unauthorized("no credentials").into_response();
    }
    next.run(req).await
}
