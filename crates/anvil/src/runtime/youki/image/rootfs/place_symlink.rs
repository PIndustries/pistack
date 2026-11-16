// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::image::place_symlink_in;
use crate::runtime::youki::image::Anchor;
use pistack_types::ApiResult;
use std::path::Path;

pub(in crate::runtime::youki) fn place_symlink(
    root: &Path,
    rel: &Path,
    target: &str,
) -> ApiResult<()> {
    let anchor = Anchor::open(root)?;
    place_symlink_in(&anchor, rel, target)
}
