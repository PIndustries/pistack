// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// Add owner read, write, and execute so a read-only unpacked directory can
/// still be deleted. Symlinks are not followed.
use crate::runtime::youki::image::set_mode_nofollow;
use std::path::Path;

pub(in crate::runtime::youki) fn chmod_writable(path: &Path) {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return;
    };
    if meta.file_type().is_symlink() || !meta.is_dir() {
        return;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = meta.permissions().mode();
        if mode & 0o700 != 0o700 {
            let _ = set_mode_nofollow(path, (mode | 0o700) & 0o7777);
        }
    }
    let Ok(rd) = std::fs::read_dir(path) else {
        return;
    };
    for ent in rd.flatten() {
        chmod_writable(&ent.path());
    }
}
