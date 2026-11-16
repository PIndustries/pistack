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

/// The container runtime writes no machine disk: an image is unpacked into a
/// `rootfs` directory and an attached volume names a host device or a folder,
/// never a disk file of its own. Nothing in its state directory is a disk a
/// cache image could be layered on.
pub(crate) fn runtime_disk_names() -> &'static [&'static str] {
    &[]
}
