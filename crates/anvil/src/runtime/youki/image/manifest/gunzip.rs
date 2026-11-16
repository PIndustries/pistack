// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::image::cannot_unpack;
use pistack_types::ApiError;
use pistack_types::ApiResult;
use std::path::Path;
use std::process::Stdio;

pub(in crate::runtime::youki) fn gunzip(src: &Path, dst: &Path) -> ApiResult<()> {
    let out = std::fs::File::create(dst)
        .map_err(|_| ApiError::unavailable("youki runtime: gzip is not available"))?;
    match std::process::Command::new("gzip")
        .arg("-dc")
        .arg(src)
        .stdout(out)
        .stderr(Stdio::null())
        .status()
    {
        Err(_) => Err(ApiError::unavailable(
            "youki runtime: gzip is not available",
        )),
        Ok(status) if status.success() => Ok(()),
        Ok(_) => Err(cannot_unpack()),
    }
}
