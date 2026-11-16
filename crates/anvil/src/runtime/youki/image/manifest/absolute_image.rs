// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::image::has_parent_dir;
use std::path::Path;
use std::path::PathBuf;

pub(in crate::runtime::youki) fn absolute_image(image: &str) -> Option<PathBuf> {
    let path = Path::new(image);
    if !path.is_absolute() || has_parent_dir(path) {
        return None;
    }
    let meta = std::fs::symlink_metadata(path).ok()?;
    if meta.file_type().is_symlink() {
        return None;
    }
    if meta.is_file() || meta.is_dir() {
        Some(path.to_path_buf())
    } else {
        None
    }
}
