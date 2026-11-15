// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::ApiError;

pub(in crate::runtime::youki) fn not_mounted_error(label: &str, path: &str) -> ApiError {
    ApiError::unavailable(format!(
        "youki runtime: slab volume {label} is not mounted at {path}"
    ))
}
