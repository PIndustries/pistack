// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::image::cannot_unpack;
use crate::runtime::youki::image::remove_path;
use pistack_types::ApiResult;
use std::path::Path;

pub(in crate::runtime::youki) fn clear_children(dir: &Path) -> ApiResult<()> {
    let rd = match std::fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err(cannot_unpack()),
    };
    for ent in rd {
        let ent = ent.map_err(|_| cannot_unpack())?;
        remove_path(&ent.path())?;
    }
    Ok(())
}
