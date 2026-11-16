// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::image::forget_path;
use crate::runtime::youki::image::Anchor;
use std::collections::BTreeMap;
use std::path::Path;
use std::path::PathBuf;

pub(in crate::runtime::youki) fn forget_tree(
    modes: &mut BTreeMap<PathBuf, u32>,
    anchor: &Anchor,
    rel: &Path,
) {
    if let Ok(key) = anchor.logical_full(rel) {
        forget_path(modes, &key);
    }
    forget_path(modes, rel);
}
