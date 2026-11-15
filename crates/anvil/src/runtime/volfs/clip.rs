// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

pub(in crate::runtime::volfs) fn clip(text: &str) -> String {
    const MAX: usize = 300;
    let flat = text.replace('\n', " ");
    if flat.chars().count() <= MAX {
        flat
    } else {
        flat.chars().take(MAX).collect()
    }
}
