// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::path::Path;
use std::path::PathBuf;

pub(in crate::runtime::youki) fn disks_path(bundle: &Path) -> PathBuf {
    bundle.join("disks.json")
}
