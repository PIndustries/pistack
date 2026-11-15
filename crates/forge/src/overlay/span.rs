// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

pub(in crate::overlay) fn span(range: Option<(u16, u16)>) -> (u16, u16) {
    match range {
        Some((min, max)) if min > 0 && max >= min => (min, max),
        _ => (1, 65535),
    }
}
