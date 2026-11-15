// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::consts::MAX_IMAGE_BYTES;
use crate::runtime::youki::image::extract_tar_budget;
use crate::runtime::youki::image::finish_dir_modes;
use pistack_types::ApiResult;
use std::collections::BTreeMap;
use std::path::Path;

pub(in crate::runtime::youki) fn extract_tar(
    path: &Path,
    dest: &Path,
    whiteouts: bool,
) -> ApiResult<()> {
    let mut budget = MAX_IMAGE_BYTES;
    let mut dir_modes = BTreeMap::new();
    extract_tar_budget(path, dest, whiteouts, &mut budget, &mut dir_modes)?;
    // The staging extract (whiteouts off) only classifies the archive.
    // Leave those directories writable so the temp dir can be removed.
    if whiteouts {
        finish_dir_modes(dest, &dir_modes)?;
    }
    Ok(())
}
