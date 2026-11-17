// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Rendering a created resource as 201 + JSON.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Serialize;

pub(super) fn created<T: Serialize>(item: T) -> Response {
    (StatusCode::CREATED, Json(item)).into_response()
}
