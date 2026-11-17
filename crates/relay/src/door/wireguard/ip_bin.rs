// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Locating the `ip` binary, or none in tests.

use super::super::binary::which;
use std::path::PathBuf;

pub(crate) fn ip_bin() -> Option<PathBuf> {
    match std::env::var_os("PISTACK_IP") {
        Some(v) if !v.is_empty() => {
            let path = PathBuf::from(v);
            path.is_file().then_some(path)
        }
        _ => {
            // Tests must not create a host interface. Production looks up `ip`.
            if cfg!(test) {
                None
            } else {
                which("ip")
            }
        }
    }
}
