// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// `PISTACK_YOUKI_CLEANUP` replaces the installed helper. An empty value is ignored.
use crate::runtime::youki::consts::CLEANUP_ENV;
use crate::runtime::youki::consts::DEFAULT_CLEANUP;
use crate::runtime::youki::is_executable::is_executable;
use std::path::PathBuf;

pub(in crate::runtime::youki) fn cleanup_helper() -> Option<PathBuf> {
    if let Some(value) = std::env::var_os(CLEANUP_ENV) {
        if !value.is_empty() {
            let path = PathBuf::from(value);
            return is_executable(&path).then_some(path);
        }
    }
    let path = PathBuf::from(DEFAULT_CLEANUP);
    is_executable(&path).then_some(path)
}
