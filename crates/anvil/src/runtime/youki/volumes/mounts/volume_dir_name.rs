// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

pub(in crate::runtime::youki) fn volume_dir_name(index: usize, name: &str) -> String {
    if name.is_empty() || name == "." || name == ".." {
        return format!("{index}-vol");
    }
    let safe: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let safe = if safe.is_empty() { "vol" } else { &safe };
    format!("{index}-{safe}")
}
