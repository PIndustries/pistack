// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::image::cannot_unpack;
use crate::runtime::youki::image::chmod_writable;
use pistack_types::ApiResult;
use std::path::Path;

pub(in crate::runtime::youki) fn remove_path(path: &Path) -> ApiResult<()> {
    let meta = match std::fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err(cannot_unpack()),
    };
    let result = if meta.file_type().is_symlink() || !meta.is_dir() {
        std::fs::remove_file(path)
    } else {
        // A layer may have left the directory without owner write.
        chmod_writable(path);
        std::fs::remove_dir_all(path)
    };
    result.map_err(|_| cannot_unpack())
}
