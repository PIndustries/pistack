// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::bundles::delete_path;
use crate::runtime::youki::privilege::Privilege;
use pistack_types::ApiError;
use pistack_types::ApiResult;
use std::path::Path;

pub(in crate::runtime::youki) fn reset_dir(dir: &Path, privilege: &Privilege) -> ApiResult<()> {
    delete_path(dir, privilege)?;
    std::fs::create_dir_all(dir).map_err(|e| {
        ApiError::unavailable(format!("youki runtime: creating {}: {e}", dir.display()))
    })
}
