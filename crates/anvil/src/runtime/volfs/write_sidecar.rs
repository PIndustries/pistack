// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::io_err::io_err;
use pistack_types::ApiResult;
use std::path::Path;

pub(in crate::runtime::volfs) fn write_sidecar(path: &Path, device: &str) -> ApiResult<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| io_err("creating", parent, err))?;
    }
    std::fs::write(path, format!("{device}\n")).map_err(|err| io_err("writing", path, err))
}
