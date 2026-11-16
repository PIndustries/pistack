// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::volumes::disks_path;
use crate::runtime::youki::volumes::read_io;
use crate::runtime::youki::volumes::AttachedDisk;
use pistack_types::ApiError;
use pistack_types::ApiResult;
use std::path::Path;

pub(in crate::runtime::youki) fn read_disks(bundle: &Path) -> ApiResult<Vec<AttachedDisk>> {
    let path = disks_path(bundle);
    match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| {
            ApiError::unavailable(format!("youki runtime: reading {}: {e}", path.display()))
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(read_io(&path, e)),
    }
}
