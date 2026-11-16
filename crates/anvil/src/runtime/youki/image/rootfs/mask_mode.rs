// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

pub(in crate::runtime::youki) fn mask_mode(mode: u32, keep_setid: bool) -> u32 {
    let mode = mode & 0o7777;
    if keep_setid {
        mode
    } else {
        mode & !0o6000
    }
}
