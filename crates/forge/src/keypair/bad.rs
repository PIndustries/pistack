// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::ApiError;

pub(crate) fn bad(msg: impl Into<String>) -> ApiError {
    ApiError::bad_request(msg)
}
