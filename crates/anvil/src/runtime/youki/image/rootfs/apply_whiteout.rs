// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::image::cannot_unpack;
use crate::runtime::youki::image::clear_children;
use crate::runtime::youki::image::forget_children;
use crate::runtime::youki::image::forget_path;
use crate::runtime::youki::image::remove_path;
use crate::runtime::youki::image::Anchor;
use pistack_types::ApiResult;
use std::collections::BTreeMap;
use std::path::Path;
use std::path::PathBuf;

pub(in crate::runtime::youki) fn apply_whiteout(
    anchor: &Anchor,
    rel: &Path,
    dir_modes: &mut BTreeMap<PathBuf, u32>,
) -> ApiResult<bool> {
    let Some(name) = rel.file_name().and_then(|s| s.to_str()) else {
        return Ok(false);
    };
    if name == ".wh..wh..opq" {
        let parent_rel = rel.parent().unwrap_or(Path::new(""));
        if parent_rel.as_os_str().is_empty() {
            clear_children(&anchor.root)?;
            dir_modes.clear();
        } else if let Some(dir) = anchor.existing(parent_rel)? {
            clear_children(&dir)?;
            if let Ok(logical) = dir.strip_prefix(&anchor.root) {
                forget_children(dir_modes, logical);
            }
            forget_children(dir_modes, parent_rel);
        }
        return Ok(true);
    }
    if let Some(rest) = name.strip_prefix(".wh.") {
        if rest.is_empty() || rest.contains('/') || rest == ".." {
            return Err(cannot_unpack());
        }
        let parent_rel = rel.parent().unwrap_or(Path::new(""));
        let target_rel = if parent_rel.as_os_str().is_empty() {
            PathBuf::from(rest)
        } else {
            parent_rel.join(rest)
        };
        if let Some(path) = anchor.existing(&target_rel)? {
            remove_path(&path)?;
        }
        if let Ok(key) = anchor.logical_full(&target_rel) {
            forget_path(dir_modes, &key);
        }
        forget_path(dir_modes, &target_rel);
        return Ok(true);
    }
    Ok(false)
}
