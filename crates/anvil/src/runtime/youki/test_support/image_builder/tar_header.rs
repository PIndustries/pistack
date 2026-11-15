// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::tar_header_full;

pub(crate) fn tar_header(name: &str, size: u64, flag: u8) -> [u8; 512] {
    tar_header_full(name, size, flag, 0o644, "")
}
