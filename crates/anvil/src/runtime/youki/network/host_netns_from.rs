// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::consts::NETNS_ENV;
use crate::runtime::youki::image::has_parent_dir;
use std::path::PathBuf;

pub(in crate::runtime::youki) fn host_netns_from(raw: Option<&std::ffi::OsStr>) -> Option<PathBuf> {
    let raw = raw?;
    if raw.is_empty() {
        return None;
    }
    let path = PathBuf::from(raw);
    if !path.is_absolute() || has_parent_dir(&path) {
        tracing::warn!(
            path = %path.display(),
            "youki runtime: ignoring {NETNS_ENV}; it must be an absolute path"
        );
        return None;
    }
    Some(path)
}
