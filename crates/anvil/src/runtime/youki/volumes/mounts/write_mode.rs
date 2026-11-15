// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::volumes::set_mode;
use crate::runtime::youki::volumes::volume_io;
use pistack_types::ApiResult;
use std::path::Path;

pub(in crate::runtime::youki) fn write_mode(path: &Path, bytes: &[u8], mode: u32) -> ApiResult<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|e| volume_io(parent, e))?;
        }
    }
    if let Ok(meta) = std::fs::symlink_metadata(path) {
        let ft = meta.file_type();
        let removed = if ft.is_symlink() || !ft.is_dir() {
            std::fs::remove_file(path)
        } else {
            std::fs::remove_dir_all(path)
        };
        removed.map_err(|e| volume_io(path, e))?;
    }
    std::fs::write(path, bytes).map_err(|e| volume_io(path, e))?;
    set_mode(path, mode)
}
