// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::ApiError;

pub(crate) fn fetch_err(e: reqwest::Error) -> ApiError {
    if e.is_timeout() || e.is_connect() {
        ApiError::unavailable(format!("image import: {e}"))
    } else {
        ApiError::bad_request(format!("image import: {e}"))
    }
}
