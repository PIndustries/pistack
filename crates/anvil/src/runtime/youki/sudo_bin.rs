// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// `PISTACK_YOUKI_SUDO_BIN` replaces `sudo` for tests. An empty value is ignored.
use crate::runtime::youki::consts::SUDO_BIN_ENV;
use crate::runtime::youki::is_executable::is_executable;
use std::path::PathBuf;

pub(super) fn sudo_bin() -> Option<PathBuf> {
    if let Some(v) = std::env::var_os(SUDO_BIN_ENV) {
        if !v.is_empty() {
            let path = PathBuf::from(v);
            return is_executable(&path).then_some(path);
        }
    }
    crate::facts::which("sudo")
}
