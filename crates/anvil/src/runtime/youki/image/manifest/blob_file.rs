// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::image::cannot_unpack;
use pistack_types::ApiResult;
use std::path::Path;
use std::path::PathBuf;

pub(in crate::runtime::youki) fn blob_file(layout: &Path, digest: &str) -> ApiResult<PathBuf> {
    let hex = digest.strip_prefix("sha256:").ok_or_else(cannot_unpack)?;
    if hex.len() < 2 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(cannot_unpack());
    }
    let hex = hex.to_ascii_lowercase();
    let base = layout.join("blobs").join("sha256");
    for path in [base.join(&hex), base.join(&hex[..2]).join(&hex)] {
        if let Ok(meta) = std::fs::symlink_metadata(&path) {
            if !meta.file_type().is_symlink() && meta.is_file() {
                return Ok(path);
            }
        }
    }
    Err(cannot_unpack())
}
