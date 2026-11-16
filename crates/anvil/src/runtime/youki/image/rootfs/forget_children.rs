// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::collections::BTreeMap;
use std::path::Path;
use std::path::PathBuf;

pub(in crate::runtime::youki) fn forget_children(modes: &mut BTreeMap<PathBuf, u32>, rel: &Path) {
    if rel.as_os_str().is_empty() {
        modes.clear();
        return;
    }
    modes.retain(|path, _| path == rel || !path.starts_with(rel));
}
