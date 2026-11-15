// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::path::Path;

pub(in crate::runtime::youki) fn rootfs_empty(rootfs: &Path) -> bool {
    std::fs::read_dir(rootfs)
        .map(|mut rd| rd.next().is_none())
        .unwrap_or(true)
}
