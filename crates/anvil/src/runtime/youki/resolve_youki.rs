// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// `$PISTACK_YOUKI_BIN` if it is set (even when that path does not exist),
/// otherwise an executable `youki` on `PATH`.
use crate::runtime::youki::is_executable::is_executable;
use pistack_types::ApiError;
use pistack_types::ApiResult;
use std::path::PathBuf;

pub(super) fn resolve_youki() -> ApiResult<PathBuf> {
    if let Some(v) = std::env::var_os("PISTACK_YOUKI_BIN") {
        if !v.is_empty() {
            let path = PathBuf::from(&v);
            if is_executable(&path) {
                return Ok(path);
            }
            return Err(ApiError::unavailable(format!(
                "youki runtime: PISTACK_YOUKI_BIN {} is not available",
                path.display()
            )));
        }
    }
    crate::facts::which("youki")
        .ok_or_else(|| ApiError::unavailable("youki runtime: youki is not on PATH"))
}
