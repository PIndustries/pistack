// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// `base` is relative to the root. `target` is the symlink text.
/// `None` means the link leaves the rootfs.
use crate::runtime::youki::image::normal_parts;
use std::ffi::OsString;
use std::path::Component;
use std::path::Path;
use std::path::PathBuf;

pub(in crate::runtime::youki) fn join_logical(base: &Path, target: &Path) -> Option<PathBuf> {
    if target.is_absolute() {
        return None;
    }
    let mut parts: Vec<OsString> = normal_parts(base);
    for comp in target.components() {
        match comp {
            Component::Normal(part) => {
                let text = part.to_string_lossy();
                if text.is_empty() || text == ".." || text.contains('/') {
                    return None;
                }
                parts.push(part.to_os_string());
            }
            Component::CurDir => {}
            Component::ParentDir => {
                parts.pop()?;
            }
            _ => return None,
        }
    }
    let mut out = PathBuf::new();
    for part in parts {
        out.push(part);
    }
    Some(out)
}
