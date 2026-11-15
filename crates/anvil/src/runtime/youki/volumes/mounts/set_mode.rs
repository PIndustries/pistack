// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::volumes::volume_io;
use pistack_types::ApiResult;
use std::path::Path;

pub(in crate::runtime::youki) fn set_mode(path: &Path, mode: u32) -> ApiResult<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(path)
            .map_err(|e| volume_io(path, e))?
            .permissions();
        perms.set_mode(mode);
        std::fs::set_permissions(path, perms).map_err(|e| volume_io(path, e))?;
    }
    #[cfg(not(unix))]
    {
        let _ = (path, mode);
    }
    Ok(())
}
