// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::consts::DATA_ENV;
use pistack_types::{ApiError, ApiResult};
use std::path::{Component, PathBuf};

pub(in crate::runtime::volfs) fn data_dir() -> ApiResult<PathBuf> {
    let raw = std::env::var_os(DATA_ENV)
        .filter(|value| !value.is_empty())
        .or_else(|| std::env::var_os("PISTACK_DATA_DIR").filter(|value| !value.is_empty()))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/var/lib/pistack"));
    if raw.is_absolute() && !raw.components().any(|c| matches!(c, Component::ParentDir)) {
        Ok(raw)
    } else {
        Err(ApiError::bad_request(
            "slab mount data dir must be an absolute path",
        ))
    }
}
