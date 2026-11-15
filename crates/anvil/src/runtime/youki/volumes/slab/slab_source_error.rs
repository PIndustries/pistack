// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::volumes::SlabSource;
use pistack_types::ApiError;

pub(in crate::runtime::youki) fn slab_source_error(
    label: &str,
    path: &str,
    kind: SlabSource,
) -> ApiError {
    let why = match kind {
        SlabSource::Missing => "does not exist",
        SlabSource::File => "is a file, not a mounted directory",
        SlabSource::Block => "is a block device, not a mounted directory",
        SlabSource::Dir | SlabSource::Other => "is not a mounted directory",
    };
    ApiError::unavailable(format!(
        "youki runtime: slab volume {label} source {path} {why}"
    ))
}
