// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::image::cannot_unpack;
use crate::runtime::youki::image::copy_children;
use pistack_types::ApiResult;
use std::path::Path;

pub(in crate::runtime::youki) fn copy_tree(src: &Path, dst: &Path) -> ApiResult<()> {
    let meta = std::fs::symlink_metadata(src).map_err(|_| cannot_unpack())?;
    if meta.file_type().is_symlink() || !meta.is_dir() {
        return Err(cannot_unpack());
    }
    std::fs::create_dir_all(dst).map_err(|_| cannot_unpack())?;
    copy_children(src, dst)
}
