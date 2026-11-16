// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::image::apply_whiteout;
use crate::runtime::youki::image::cannot_unpack;
use crate::runtime::youki::image::forget_tree;
use crate::runtime::youki::image::keep_setid;
use crate::runtime::youki::image::mask_mode;
use crate::runtime::youki::image::open_nofollow_new;
use crate::runtime::youki::image::place_hardlink;
use crate::runtime::youki::image::place_symlink_in;
use crate::runtime::youki::image::rel_under;
use crate::runtime::youki::image::set_file_mode;
use crate::runtime::youki::image::Anchor;
use crate::runtime::youki::image::TarReader;
use pistack_types::ApiResult;
use std::collections::BTreeMap;
use std::path::Path;
use std::path::PathBuf;

pub(in crate::runtime::youki) fn extract_tar_budget(
    path: &Path,
    dest: &Path,
    whiteouts: bool,
    budget: &mut u64,
    dir_modes: &mut BTreeMap<PathBuf, u32>,
) -> ApiResult<()> {
    let anchor = Anchor::open(dest)?;
    let file = std::fs::File::open(path).map_err(|_| cannot_unpack())?;
    let mut tar = TarReader {
        inner: file,
        pending_name: None,
        pending_link: None,
    };
    let privileged = keep_setid();
    while let Some(ent) = tar.next_entry()? {
        let rel = match rel_under(&ent.name) {
            Some(rel) => rel,
            None if ent.name.is_empty() || ent.name == "." || ent.name == "./" => {
                tar.discard(ent.size)?;
                continue;
            }
            None => return Err(cannot_unpack()),
        };
        let mode = mask_mode(ent.mode, privileged);
        if whiteouts && apply_whiteout(&anchor, &rel, dir_modes)? {
            tar.discard(ent.size)?;
            continue;
        }
        match ent.flag {
            b'5' => {
                tar.discard(ent.size)?;
                anchor.ensure_dir(&rel)?;
                let key = anchor.logical_full(&rel)?;
                if key != rel {
                    dir_modes.remove(&rel);
                }
                dir_modes.insert(key, mode);
            }
            b'2' => {
                tar.discard(ent.size)?;
                forget_tree(dir_modes, &anchor, &rel);
                place_symlink_in(&anchor, &rel, &ent.link)?;
            }
            b'1' => {
                tar.discard(ent.size)?;
                forget_tree(dir_modes, &anchor, &rel);
                place_hardlink(&anchor, &rel, &ent.link)?;
            }
            b'0' | 0 => {
                forget_tree(dir_modes, &anchor, &rel);
                let dest_file = anchor.prepare_file(&rel)?;
                let mut out = open_nofollow_new(&dest_file)?;
                tar.copy_entry(ent.size, &mut out, budget)?;
                set_file_mode(&out, mode)?;
            }
            _ => tar.discard(ent.size)?,
        }
    }
    Ok(())
}
