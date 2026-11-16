// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::path::Component;
use std::path::Path;

pub(in crate::runtime::youki) fn removal_text_ok(path: &Path) -> bool {
    let text = path.to_string_lossy();
    if text.contains('\0') || text.chars().any(|ch| ch.is_control() || ch == '\u{7f}') {
        return false;
    }
    !text.split('/').any(|part| part == "..")
        && !path
            .components()
            .any(|component| matches!(component, Component::ParentDir))
}
