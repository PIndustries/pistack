// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::path::PathBuf;

pub(crate) fn install_cleanup_helper(dir: &Path) -> PathBuf {
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../scripts/pistack-youki-cleanup");
    let dest = dir.join("pistack-youki-cleanup");
    std::fs::copy(&src, &dest).unwrap();
    let mut perms = std::fs::metadata(&dest).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&dest, perms).unwrap();
    dest
}
