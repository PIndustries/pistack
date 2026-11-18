// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::ApiError;

/// The error a caller is owed when the key holds no record at all.
pub(crate) fn key_not_there(key: &str) -> ApiError {
    ApiError::not_found(format!("object '{key}' not found"))
}
