// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::volumes::classify_slab_path;
use crate::runtime::youki::volumes::slab_source_error;
use crate::runtime::youki::volumes::slab_source_io;
/// `disks.json` recorded a backing file or device. That is not the bind.
use crate::runtime::youki::volumes::SlabSource;
use pistack_types::ApiError;

pub(in crate::runtime::youki) fn raw_source_error(label: &str, path: &str) -> ApiError {
    match classify_slab_path(path) {
        Ok(SlabSource::Dir) | Ok(SlabSource::Other) => {
            slab_source_error(label, path, SlabSource::Other)
        }
        Ok(kind) => slab_source_error(label, path, kind),
        Err(err) => slab_source_io(label, path, err),
    }
}
