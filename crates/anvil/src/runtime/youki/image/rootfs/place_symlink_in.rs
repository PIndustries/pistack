// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::image::cannot_unpack;
use crate::runtime::youki::image::remove_path;
use crate::runtime::youki::image::symlink_stays;
use crate::runtime::youki::image::Anchor;
use pistack_types::ApiResult;
use std::path::Path;

pub(in crate::runtime::youki) fn place_symlink_in(
    anchor: &Anchor,
    rel: &Path,
    target: &str,
) -> ApiResult<()> {
    if !symlink_stays(rel, target) {
        return Err(cannot_unpack());
    }
    let (logical, name) = anchor.split(rel, true)?.ok_or_else(cannot_unpack)?;
    let dest = anchor.abs(&logical).join(name);
    remove_path(&dest)?;
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, &dest).map_err(|_| cannot_unpack())
    }
    #[cfg(not(unix))]
    {
        let _ = (dest, target);
        Err(cannot_unpack())
    }
}
