// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::image::cannot_unpack;
use crate::runtime::youki::image::join_logical;
/// True when `path` and every symlink it names stay inside the root.
use crate::runtime::youki::image::Anchor;
use pistack_types::ApiResult;
use std::path::Path;

pub(in crate::runtime::youki) fn chain_stays(anchor: &Anchor, path: &Path) -> ApiResult<bool> {
    let mut current = path.to_path_buf();
    for _ in 0..40 {
        let meta = std::fs::symlink_metadata(&current).map_err(|_| cannot_unpack())?;
        if !meta.file_type().is_symlink() {
            return Ok(current.starts_with(&anchor.root));
        }
        let target = std::fs::read_link(&current).map_err(|_| cannot_unpack())?;
        let Some(parent) = current.parent() else {
            return Ok(false);
        };
        let Ok(parent_rel) = parent.strip_prefix(&anchor.root) else {
            return Ok(false);
        };
        let Some(next_rel) = join_logical(parent_rel, &target) else {
            return Ok(false);
        };
        current = anchor.abs(&next_rel);
    }
    Ok(false)
}
