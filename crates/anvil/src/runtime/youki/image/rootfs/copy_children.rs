// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::image::cannot_unpack;
use crate::runtime::youki::image::copy_file;
use crate::runtime::youki::image::place_symlink;
use crate::runtime::youki::image::set_mode_nofollow;
use crate::runtime::youki::image::source_mode;
use pistack_types::ApiResult;
use std::path::Path;

pub(in crate::runtime::youki) fn copy_children(src: &Path, dst: &Path) -> ApiResult<()> {
    for ent in std::fs::read_dir(src).map_err(|_| cannot_unpack())? {
        let ent = ent.map_err(|_| cannot_unpack())?;
        let name = ent.file_name();
        let name_str = name.to_string_lossy();
        if name_str.is_empty() || name_str.contains('/') || name_str == ".." || name_str == "." {
            return Err(cannot_unpack());
        }
        let from = ent.path();
        let to = dst.join(&name);
        let meta = std::fs::symlink_metadata(&from).map_err(|_| cannot_unpack())?;
        if meta.file_type().is_symlink() {
            let target = std::fs::read_link(&from).map_err(|_| cannot_unpack())?;
            let target = target.to_string_lossy().into_owned();
            let name = name.to_string_lossy().into_owned();
            place_symlink(dst, Path::new(&name), &target)?;
        } else if meta.is_dir() {
            std::fs::create_dir_all(&to).map_err(|_| cannot_unpack())?;
            copy_children(&from, &to)?;
            // After the children, same as a tar directory. A mode without
            // owner write must not block the copy that just finished.
            set_mode_nofollow(&to, source_mode(&meta))?;
        } else if meta.is_file() {
            copy_file(&from, &to, source_mode(&meta))?;
        }
    }
    Ok(())
}
