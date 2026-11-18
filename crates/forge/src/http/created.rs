// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::Serialize;

pub(crate) fn created<T: Serialize>(v: T) -> Response {
    (StatusCode::CREATED, Json(v)).into_response()
}
