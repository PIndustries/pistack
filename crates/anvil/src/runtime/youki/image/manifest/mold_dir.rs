// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::path::PathBuf;

pub(in crate::runtime::youki) fn mold_dir() -> PathBuf {
    if let Some(v) = std::env::var_os("PISTACK_MOLD_DIR") {
        if !v.is_empty() {
            return PathBuf::from(v);
        }
    }
    if let Some(v) = std::env::var_os("PISTACK_DATA_DIR") {
        if !v.is_empty() {
            return PathBuf::from(v).join("images");
        }
    }
    PathBuf::from("data/images")
}
