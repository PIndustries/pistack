// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::path::Path;

pub(in crate::runtime::youki) fn path_exists(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok()
}
