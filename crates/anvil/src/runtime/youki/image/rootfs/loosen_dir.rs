// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::image::cannot_unpack;
use crate::runtime::youki::image::set_mode_nofollow;
use pistack_types::ApiResult;
use std::path::Path;

pub(in crate::runtime::youki) fn loosen_dir(path: &Path) -> ApiResult<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let meta = std::fs::symlink_metadata(path).map_err(|_| cannot_unpack())?;
        if meta.file_type().is_symlink() || !meta.is_dir() {
            return Err(cannot_unpack());
        }
        let mode = meta.permissions().mode();
        if mode & 0o700 != 0o700 {
            set_mode_nofollow(path, (mode | 0o700) & 0o7777)?;
        }
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}
