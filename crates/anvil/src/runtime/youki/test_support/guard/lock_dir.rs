// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::os::unix::fs::PermissionsExt;
use std::path::Path;

pub(crate) fn lock_dir(path: &Path) {
    std::fs::create_dir_all(path).unwrap();
    std::fs::write(path.join("file"), b"x").unwrap();
    let mut perms = std::fs::metadata(path).unwrap().permissions();
    perms.set_mode(0o0);
    std::fs::set_permissions(path, perms).unwrap();
}
