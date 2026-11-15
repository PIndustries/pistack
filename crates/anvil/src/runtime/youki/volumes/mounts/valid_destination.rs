// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::image::has_parent_dir;
use std::path::Path;

pub(in crate::runtime::youki) fn valid_destination(path: &str) -> bool {
    if path.len() < 2 || !path.starts_with('/') || path.contains('\0') {
        return false;
    }
    if path == "/proc" || path == "/dev" || path == "/sys" {
        return false;
    }
    !has_parent_dir(Path::new(path))
}
