// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Whether a request head carries a header of this name.

pub(super) fn header_has(head: &[u8], name: &str) -> bool {
    let Ok(text) = std::str::from_utf8(head) else {
        return false;
    };
    text.lines().skip(1).any(|line| {
        line.split(':')
            .next()
            .is_some_and(|h| h.trim().eq_ignore_ascii_case(name))
    })
}
