// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::image::has_parent_dir;
use crate::runtime::youki::volumes::is_block_device;
use crate::runtime::youki::volumes::SlabSource;
use std::path::Path;

pub(in crate::runtime::youki) fn classify_slab_path(path: &str) -> std::io::Result<SlabSource> {
    let p = Path::new(path);
    if path.is_empty() || path.contains('\0') || !p.is_absolute() || has_parent_dir(p) {
        return Ok(SlabSource::Other);
    }
    match std::fs::metadata(p) {
        Ok(meta) if meta.is_dir() => Ok(SlabSource::Dir),
        Ok(meta) if is_block_device(&meta) => Ok(SlabSource::Block),
        Ok(meta) if meta.is_file() => Ok(SlabSource::File),
        Ok(_) => Ok(SlabSource::Other),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(SlabSource::Missing),
        Err(err) => Err(err),
    }
}
