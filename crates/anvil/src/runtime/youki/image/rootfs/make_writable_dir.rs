// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::image::cannot_unpack;
use crate::runtime::youki::image::set_mode_nofollow;
use pistack_types::ApiResult;
use std::path::Path;

pub(in crate::runtime::youki) fn make_writable_dir(path: &Path) -> ApiResult<()> {
    std::fs::create_dir(path).map_err(|_| cannot_unpack())?;
    set_mode_nofollow(path, 0o755)
}
