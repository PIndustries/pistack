// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::super::EnvSlot;
use crate::runtime::youki::consts::MOUNTINFO_ENV;
use std::path::Path;

/// Mount table the slab checks read instead of `/proc/self/mountinfo`.
pub(crate) fn inject_mountinfo(dir: &Path, mounts: &[&Path]) -> EnvSlot {
    let file = dir.join("mountinfo");
    let mut text = String::new();
    for (index, mount) in mounts.iter().enumerate() {
        let id = index + 11;
        let point = mount.to_string_lossy().replace(' ', "\\040");
        text.push_str(&format!(
            "{id} 1 8:{id} / {point} rw - ext4 /dev/slab{id} rw\n"
        ));
    }
    std::fs::write(&file, text).unwrap();
    EnvSlot::set(MOUNTINFO_ENV, file.to_str().expect("utf8"))
}
