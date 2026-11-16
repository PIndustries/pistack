// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::image::cannot_unpack;
use crate::runtime::youki::image::chain_stays;
use crate::runtime::youki::image::rel_under;
use crate::runtime::youki::image::remove_path;
use crate::runtime::youki::image::Anchor;
use pistack_types::ApiResult;
use std::path::Path;

pub(in crate::runtime::youki) fn place_hardlink(
    anchor: &Anchor,
    rel: &Path,
    target: &str,
) -> ApiResult<()> {
    let target_rel = rel_under(target).ok_or_else(cannot_unpack)?;
    let Some(src) = anchor.existing(&target_rel)? else {
        return Err(cannot_unpack());
    };
    if !chain_stays(anchor, &src)? {
        return Err(cannot_unpack());
    }
    let (logical, name) = anchor.split(rel, true)?.ok_or_else(cannot_unpack)?;
    let dest = anchor.abs(&logical).join(name);
    remove_path(&dest)?;
    std::fs::hard_link(src, dest).map_err(|_| cannot_unpack())
}
