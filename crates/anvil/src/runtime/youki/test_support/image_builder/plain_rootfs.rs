// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::super::req;
use crate::runtime::MachineRequest;
use std::path::Path;

pub(crate) fn plain_rootfs(tmp: &Path) -> MachineRequest {
    let image = tmp.join("rootfs");
    std::fs::create_dir_all(&image).unwrap();
    std::fs::write(image.join("marker"), b"keep").unwrap();
    req("web", image.to_str().unwrap(), "10.4.0.1")
}
