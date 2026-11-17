// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

pub(crate) fn ignored_label(kind_name: &str, name: &str) -> String {
    let name = name.trim();
    if name.is_empty() {
        format!("kind {kind_name}")
    } else {
        format!("kind {kind_name} ({name})")
    }
}
