// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::consts::MOUNTINFO_ENV;
use std::path::PathBuf;

pub(in crate::runtime::youki) fn mountinfo_file() -> PathBuf {
    match std::env::var_os(MOUNTINFO_ENV) {
        Some(value) if !value.is_empty() => PathBuf::from(value),
        _ => PathBuf::from("/proc/self/mountinfo"),
    }
}
