// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::path::Path;

pub(in crate::runtime::youki) fn path_arg(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}
