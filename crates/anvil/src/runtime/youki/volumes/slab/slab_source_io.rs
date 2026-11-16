// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::ApiError;

pub(in crate::runtime::youki) fn slab_source_io(
    label: &str,
    path: &str,
    err: std::io::Error,
) -> ApiError {
    ApiError::unavailable(format!(
        "youki runtime: slab volume {label} source {path}: {err}"
    ))
}
