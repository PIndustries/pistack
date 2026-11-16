// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::ApiError;
use std::path::Path;

pub(in crate::runtime::volfs) fn io_err(
    action: &str,
    path: &Path,
    err: std::io::Error,
) -> ApiError {
    ApiError::unavailable(format!("{action} {}: {err}", path.display()))
}
