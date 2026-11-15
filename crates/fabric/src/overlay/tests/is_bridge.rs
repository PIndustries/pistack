// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

pub(crate) fn is_bridge(name: &str) -> bool {
    let b = name.as_bytes();
    b.len() == 14
        && b.starts_with(b"po")
        && b[2..]
            .iter()
            .all(|c| matches!(*c, b'0'..=b'9' | b'a'..=b'f'))
}
