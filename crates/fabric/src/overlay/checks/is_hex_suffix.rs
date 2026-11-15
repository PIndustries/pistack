// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

pub(in crate::overlay) fn is_hex_suffix(name: &str, prefix: &str) -> bool {
    match name.strip_prefix(prefix) {
        Some(rest) => {
            rest.len() == 12 && rest.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        }
        None => false,
    }
}
