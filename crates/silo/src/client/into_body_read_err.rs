// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::ApiError;

pub trait IntoBodyReadErr {
    fn into_body_read_err(self) -> ApiError;
}

impl IntoBodyReadErr for ApiError {
    fn into_body_read_err(self) -> ApiError {
        self
    }
}

impl IntoBodyReadErr for std::io::Error {
    fn into_body_read_err(self) -> ApiError {
        ApiError::bad_request(format!("reading body: {self}"))
    }
}

impl IntoBodyReadErr for String {
    fn into_body_read_err(self) -> ApiError {
        ApiError::bad_request(format!("reading body: {self}"))
    }
}

impl IntoBodyReadErr for &str {
    fn into_body_read_err(self) -> ApiError {
        ApiError::bad_request(format!("reading body: {self}"))
    }
}

impl IntoBodyReadErr for axum::Error {
    fn into_body_read_err(self) -> ApiError {
        ApiError::bad_request(format!("reading body: {self}"))
    }
}
