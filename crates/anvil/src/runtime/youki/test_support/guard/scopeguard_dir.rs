// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::DirGuard;
use std::path::Path;

pub(crate) fn scopeguard_dir(path: &Path) -> DirGuard {
    DirGuard(path.to_path_buf())
}
