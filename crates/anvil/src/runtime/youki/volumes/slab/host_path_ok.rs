// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::image::has_parent_dir;
use pistack_types::ApiError;
use pistack_types::ApiResult;
use std::path::Path;

pub(in crate::runtime::youki) fn host_path_ok(path: &str) -> ApiResult<()> {
    let p = Path::new(path);
    if path.is_empty() || path.contains('\0') || !p.is_absolute() || has_parent_dir(p) {
        return Err(ApiError::bad_request(
            "volume path must be an absolute path",
        ));
    }
    Ok(())
}
