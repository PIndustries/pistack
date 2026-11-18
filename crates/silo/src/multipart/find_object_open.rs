// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

pub(crate) fn find_object_open(s: &str) -> Option<usize> {
    for pat in ["<Object>", "<Object "] {
        if let Some(i) = s.find(pat) {
            return Some(i);
        }
    }
    None
}
