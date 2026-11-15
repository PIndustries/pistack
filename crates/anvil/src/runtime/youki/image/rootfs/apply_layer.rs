// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::image::decompress_to_tar;
use crate::runtime::youki::image::extract_tar_budget;
use pistack_types::ApiResult;
use std::collections::BTreeMap;
use std::path::Path;
use std::path::PathBuf;

pub(in crate::runtime::youki) fn apply_layer(
    path: &Path,
    rootfs: &Path,
    budget: &mut u64,
    dir_modes: &mut BTreeMap<PathBuf, u32>,
) -> ApiResult<()> {
    let owned = decompress_to_tar(path)?;
    extract_tar_budget(&owned.path, rootfs, true, budget, dir_modes)
}
