// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::image::chmod_deletable;
use std::path::Path;

pub(in crate::runtime::youki) fn remove_unprivileged(path: &Path) -> std::io::Result<()> {
    let meta = match std::fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    if meta.file_type().is_symlink() || !meta.is_dir() {
        std::fs::remove_file(path)
    } else {
        // A layer directory such as 0555 is still ours to delete when this
        // uid can already access it. Mode 000 stays locked, and a directory
        // we do not own is unchanged when chmod fails. Sudo mode removes
        // those through the cleanup helper.
        chmod_deletable(path);
        std::fs::remove_dir_all(path)
    }
}
