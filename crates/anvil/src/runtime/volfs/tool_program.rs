// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::path::PathBuf;

pub(in crate::runtime::volfs) fn tool_program(env_key: &str, default: &str) -> PathBuf {
    if let Ok(raw) = std::env::var(env_key) {
        let raw = raw.trim();
        if !raw.is_empty() && !raw.contains('\0') {
            return PathBuf::from(raw);
        }
    }
    PathBuf::from(default)
}
