// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::image::cannot_unpack;
use crate::runtime::youki::image::set_file_mode;
use pistack_types::ApiResult;
use std::path::Path;

pub(in crate::runtime::youki) fn copy_file(from: &Path, to: &Path, mode: u32) -> ApiResult<()> {
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent).map_err(|_| cannot_unpack())?;
    }
    let mut input = std::fs::File::open(from).map_err(|_| cannot_unpack())?;
    let mut output = std::fs::File::create(to).map_err(|_| cannot_unpack())?;
    std::io::copy(&mut input, &mut output).map_err(|_| cannot_unpack())?;
    set_file_mode(&output, mode)
}
