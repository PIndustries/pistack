// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::image::has_parent_dir;
use std::path::Component;
use std::path::Path;

pub(in crate::runtime::youki) fn symlink_stays(link_rel: &Path, target: &str) -> bool {
    let path = Path::new(target);
    if path.is_absolute() {
        return !has_parent_dir(path);
    }
    let mut depth = link_rel
        .parent()
        .unwrap_or(Path::new(""))
        .components()
        .filter(|c| matches!(c, Component::Normal(_)))
        .count() as i32;
    for c in path.components() {
        match c {
            Component::Normal(_) => depth += 1,
            Component::CurDir => {}
            Component::ParentDir => {
                depth -= 1;
                if depth < 0 {
                    return false;
                }
            }
            _ => return false,
        }
    }
    true
}
