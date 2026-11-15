// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// `$PISTACK_YOUKI_STATE`, else `$PISTACK_DATA_DIR/youki`, else
/// `/var/lib/pistack/youki`. An empty variable is treated as unset.
use crate::runtime::youki::consts::DEFAULT_STATE;
use std::path::PathBuf;

pub(crate) fn state_dir() -> PathBuf {
    if let Some(v) = std::env::var_os("PISTACK_YOUKI_STATE") {
        if !v.is_empty() {
            return PathBuf::from(v);
        }
    }
    if let Some(v) = std::env::var_os("PISTACK_DATA_DIR") {
        if !v.is_empty() {
            return PathBuf::from(v).join("youki");
        }
    }
    PathBuf::from(DEFAULT_STATE)
}
