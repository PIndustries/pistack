// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::ApiError;

pub(crate) fn io_err(e: std::io::Error) -> ApiError {
    ApiError::internal(format!("silo: {e}"))
}
