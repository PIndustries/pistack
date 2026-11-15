// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::volumes::classify_slab_path;
use crate::runtime::youki::volumes::folder_is_mount_point;
use crate::runtime::youki::volumes::not_mounted_error;
use crate::runtime::youki::volumes::slab_source_error;
use crate::runtime::youki::volumes::slab_source_io;
/// The mounted folder, or an error that names the volume.
use crate::runtime::youki::volumes::SlabSource;
use pistack_types::ApiResult;
use std::path::PathBuf;

pub(in crate::runtime::youki) fn require_mounted_dir(
    label: &str,
    path: &str,
) -> ApiResult<PathBuf> {
    match classify_slab_path(path) {
        Ok(SlabSource::Dir) => match folder_is_mount_point(path) {
            Ok(true) => Ok(PathBuf::from(path)),
            Ok(false) => Err(not_mounted_error(label, path)),
            Err(err) => Err(slab_source_io(label, path, err)),
        },
        Ok(kind) => Err(slab_source_error(label, path, kind)),
        Err(err) => Err(slab_source_io(label, path, err)),
    }
}
