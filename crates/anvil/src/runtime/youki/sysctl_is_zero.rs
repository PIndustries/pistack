// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

pub(super) fn sysctl_is_zero(path: &str) -> bool {
    std::fs::read_to_string(path)
        .map(|text| text.trim() == "0")
        .unwrap_or(false)
}
