// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::path::PathBuf;

pub struct PreparedVolume {
    pub(in crate::runtime::youki) host_path: PathBuf,
    pub(in crate::runtime::youki) directory: bool,
    pub(in crate::runtime::youki) force_read_only: bool,
}
