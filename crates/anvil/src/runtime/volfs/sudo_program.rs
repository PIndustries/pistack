// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::consts::{SUDOERS_SAMPLE, SUDO_BIN_ENV, SUDO_ENV};
use super::executable::executable;
use pistack_types::{ApiError, ApiResult};
use std::path::PathBuf;

pub(in crate::runtime::volfs) fn sudo_program() -> ApiResult<PathBuf> {
    if let Some(raw) = std::env::var_os(SUDO_BIN_ENV) {
        if !raw.is_empty() {
            let path = PathBuf::from(raw);
            if executable(&path) {
                return Ok(path);
            }
            return Err(ApiError::unavailable(format!(
                "slab mount: {SUDO_ENV} is set but sudo is not available. Install sudo and `{SUDOERS_SAMPLE}`"
            )));
        }
    }
    crate::facts::which("sudo").ok_or_else(|| {
        ApiError::unavailable(format!(
            "slab mount: {SUDO_ENV} is set but sudo is not available. Install sudo and `{SUDOERS_SAMPLE}`"
        ))
    })
}
