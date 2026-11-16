// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::ApiError;
use std::path::Path;

pub(in crate::runtime::youki) fn remove_err(path: &Path, err: &std::io::Error) -> ApiError {
    ApiError::unavailable(format!("youki runtime: removing {}: {err}", path.display()))
}
