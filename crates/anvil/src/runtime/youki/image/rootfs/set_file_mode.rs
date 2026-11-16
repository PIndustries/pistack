// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::image::cannot_unpack;
use pistack_types::ApiResult;

pub(in crate::runtime::youki) fn set_file_mode(file: &std::fs::File, mode: u32) -> ApiResult<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = file.metadata().map_err(|_| cannot_unpack())?.permissions();
        perms.set_mode(mode & 0o7777);
        file.set_permissions(perms).map_err(|_| cannot_unpack())?;
    }
    #[cfg(not(unix))]
    {
        let _ = (file, mode);
    }
    Ok(())
}
