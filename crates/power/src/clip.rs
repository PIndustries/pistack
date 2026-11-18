// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// One line of tool stderr, flattened and clipped, for an error message.
pub(crate) fn clip(s: &str) -> String {
    let flat: String = s
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let trimmed = flat.trim();
    if trimmed.len() > 180 {
        format!("{}…", &trimmed[..180])
    } else {
        trimmed.to_owned()
    }
}
