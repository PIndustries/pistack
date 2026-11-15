// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::path::Component;
use std::path::Path;
use std::path::PathBuf;

pub(in crate::runtime::youki) fn rel_under(name: &str) -> Option<PathBuf> {
    if name.contains('\0') {
        return None;
    }
    let mut out = PathBuf::new();
    for c in Path::new(name).components() {
        match c {
            Component::Normal(s) => {
                let text = s.to_string_lossy();
                if text.is_empty() || text == ".." || text.contains('/') {
                    return None;
                }
                out.push(s);
            }
            Component::CurDir | Component::RootDir => {}
            Component::ParentDir | Component::Prefix(_) => return None,
        }
    }
    if out.as_os_str().is_empty() {
        None
    } else {
        Some(out)
    }
}
