// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

pub(in crate::runtime::volfs) fn loop_device_ok(path: &str) -> bool {
    let Some(rest) = path.strip_prefix("/dev/loop") else {
        return false;
    };
    !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit()) && path.len() < 64
}
