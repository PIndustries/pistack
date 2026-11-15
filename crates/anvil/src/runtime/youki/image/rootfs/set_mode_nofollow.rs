// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::image::cannot_unpack;
use pistack_types::ApiResult;
use std::path::Path;

pub(in crate::runtime::youki) fn set_mode_nofollow(path: &Path, mode: u32) -> ApiResult<()> {
    let meta = std::fs::symlink_metadata(path).map_err(|_| cannot_unpack())?;
    // chmod follows a final symlink. Refuse that rather than changing a
    // target that may sit outside the rootfs. Callers pass a path whose
    // parents were already resolved inside the root.
    if meta.file_type().is_symlink() {
        return Err(cannot_unpack());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = meta.permissions();
        perms.set_mode(mode & 0o7777);
        std::fs::set_permissions(path, perms).map_err(|_| cannot_unpack())?;
    }
    #[cfg(not(unix))]
    {
        let _ = mode;
    }
    Ok(())
}
