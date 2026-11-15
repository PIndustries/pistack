// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::image::cannot_unpack;
use crate::runtime::youki::image::rel_under;
use pistack_types::ApiResult;
use std::path::PathBuf;

pub(in crate::runtime::youki) fn safe_rel(name: &str) -> ApiResult<PathBuf> {
    rel_under(name).ok_or_else(cannot_unpack)
}
