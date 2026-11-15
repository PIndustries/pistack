// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// Loosen a directory this process can already access so `remove_dir_all`
/// can unlink its children. Mode 000 is left unchanged: there is no owner
/// bit to extend, and sudo mode hands that tree to the cleanup helper.
/// Symlinks are not followed. A chmod that fails (the directory is not
/// ours) is ignored.
use crate::runtime::youki::image::set_mode_nofollow;
use std::path::Path;

pub(in crate::runtime::youki) fn chmod_deletable(path: &Path) {
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
        if mode & 0o700 != 0 && mode & 0o700 != 0o700 {
            let _ = set_mode_nofollow(path, (mode | 0o700) & 0o7777);
        }
    }
    let Ok(rd) = std::fs::read_dir(path) else {
        return;
    };
    for ent in rd.flatten() {
        chmod_deletable(&ent.path());
    }
}
