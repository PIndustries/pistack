// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The reply to a create: 201 with the resource in the body.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Serialize;

pub(super) fn created<T: Serialize>(t: T) -> Response {
    (StatusCode::CREATED, Json(t)).into_response()
}
