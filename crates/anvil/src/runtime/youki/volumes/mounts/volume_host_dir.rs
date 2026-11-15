// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::volumes::volume_dir_name;
use std::path::Path;
use std::path::PathBuf;

pub(in crate::runtime::youki) fn volume_host_dir(pod: &Path, index: usize, name: &str) -> PathBuf {
    pod.join("volumes").join(volume_dir_name(index, name))
}
