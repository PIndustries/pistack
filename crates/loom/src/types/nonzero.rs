// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

// ---- kinds and well-known keys ------------------------------------------------

pub(crate) fn nonzero(value: u32, default: u32) -> u32 {
    if value == 0 {
        default
    } else {
        value
    }
}
