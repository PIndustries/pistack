// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::io_err::io_err;
use pistack_types::{ApiError, ApiResult};
use std::path::Path;

pub(in crate::runtime::volfs) fn prepare_mount_dir(dir: &Path) -> ApiResult<()> {
    if let Ok(meta) = std::fs::symlink_metadata(dir) {
        if meta.file_type().is_symlink() {
            return Err(ApiError::unavailable(format!(
                "mount directory {} is a symlink",
                dir.display()
            )));
        }
    }
    if let Some(parent) = dir.parent() {
        std::fs::create_dir_all(parent).map_err(|err| io_err("creating", parent, err))?;
    }
    if !dir.exists() {
        std::fs::create_dir(dir).map_err(|err| io_err("creating", dir, err))?;
    }
    Ok(())
}
