// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// Whether `path` is a mount point.
///
/// The mount table is preferred. The path has to appear as a mount point,
/// after octal escapes such as `\040` are turned back into characters. A
/// bind on the same disk does not change the device number, so the table
/// is what catches that. When the table cannot be read, the folder's
/// device number is compared with its parent's.
use crate::runtime::youki::volumes::mounted_by_device;
use crate::runtime::youki::volumes::mountinfo_file;
use crate::runtime::youki::volumes::mountinfo_lists;
use std::path::Path;

pub(in crate::runtime::youki) fn folder_is_mount_point(path: &str) -> std::io::Result<bool> {
    match std::fs::read_to_string(mountinfo_file()) {
        Ok(text) => Ok(mountinfo_lists(&text, path)),
        Err(_) => mounted_by_device(Path::new(path)),
    }
}
