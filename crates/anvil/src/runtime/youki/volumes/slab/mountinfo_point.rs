// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// Field 5 of a mountinfo line is the mount point. Optional fields sit
/// after the mount options, so the point stays the fifth word.
use crate::runtime::youki::volumes::unescape_mount;

pub(in crate::runtime::youki) fn mountinfo_point(line: &str) -> Option<String> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    line.split_whitespace().nth(4).map(unescape_mount)
}
