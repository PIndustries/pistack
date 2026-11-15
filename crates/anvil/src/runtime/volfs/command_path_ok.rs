// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::path::{Component, Path};

pub(in crate::runtime::volfs) fn command_path_ok(path: &str) -> bool {
    let p = Path::new(path);
    !path.is_empty()
        && path.len() <= 4096
        && !path.contains('\0')
        && !path.chars().any(|c| c.is_whitespace() || c.is_control())
        && p.is_absolute()
        && !p.components().any(|c| matches!(c, Component::ParentDir))
}
