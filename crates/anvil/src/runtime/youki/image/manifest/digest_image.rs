// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::image::has_parent_dir;
use crate::runtime::youki::image::mold_dir;
use std::path::PathBuf;

pub(in crate::runtime::youki) fn digest_image(image: &str) -> Option<PathBuf> {
    let hex = image.strip_prefix("sha256:")?;
    if hex.len() < 2 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let hex = hex.to_ascii_lowercase();
    let base = mold_dir().join("blobs").join("sha256");
    if has_parent_dir(&base) {
        return None;
    }
    for path in [base.join(&hex[..2]).join(&hex), base.join(&hex)] {
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if meta.file_type().is_symlink() {
            continue;
        }
        if meta.is_file() || meta.is_dir() {
            return Some(path);
        }
    }
    None
}
