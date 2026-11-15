// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::image::cannot_unpack;
use crate::runtime::youki::image::set_mode_nofollow;
use crate::runtime::youki::image::Anchor;
use pistack_types::ApiResult;
use std::collections::BTreeMap;
use std::path::Path;
use std::path::PathBuf;

pub(in crate::runtime::youki) fn finish_dir_modes(
    dest: &Path,
    dir_modes: &BTreeMap<PathBuf, u32>,
) -> ApiResult<()> {
    if dir_modes.is_empty() {
        return Ok(());
    }
    let anchor = Anchor::open(dest)?;
    let mut entries: Vec<(&Path, u32)> = dir_modes.iter().map(|(p, m)| (p.as_path(), *m)).collect();
    // Deepest first, while parent directories are still searchable.
    entries.sort_by_key(|(path, _)| std::cmp::Reverse(path.components().count()));
    for (rel, mode) in entries {
        let Some(path) = anchor.existing(rel)? else {
            continue;
        };
        let meta = std::fs::symlink_metadata(&path).map_err(|_| cannot_unpack())?;
        if meta.file_type().is_symlink() || !meta.is_dir() {
            continue;
        }
        set_mode_nofollow(&path, mode)?;
    }
    Ok(())
}
