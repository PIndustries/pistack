// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::{ApiError, ApiResult};
use std::path::PathBuf;

use crate::which::which;

pub(crate) fn tool(env: &str, name: &str) -> ApiResult<PathBuf> {
    match std::env::var_os(env) {
        Some(v) if !v.is_empty() => {
            let path = PathBuf::from(v);
            if path.is_file() {
                Ok(path)
            } else {
                Err(ApiError::unavailable(format!("{name} is not installed")))
            }
        }
        Some(_) => Err(ApiError::unavailable(format!("{name} is not installed"))),
        None => {
            which(name).ok_or_else(|| ApiError::unavailable(format!("{name} is not installed")))
        }
    }
}
