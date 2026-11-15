// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::path::Component;
use std::path::Path;

pub(in crate::runtime::youki) fn has_parent_dir(path: &Path) -> bool {
    path.components().any(|c| matches!(c, Component::ParentDir))
        || path.to_string_lossy().split('/').any(|p| p == "..")
}
