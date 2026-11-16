// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

pub(in crate::probe) fn host_ok(host: &str) -> bool {
    !host.is_empty()
        && !host
            .chars()
            .any(|c| c.is_ascii_control() || c == ' ' || c == '/')
        && host.len() <= 253
}
