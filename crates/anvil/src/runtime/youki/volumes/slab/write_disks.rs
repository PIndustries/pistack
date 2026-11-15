// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::volumes::disks_path;
use crate::runtime::youki::volumes::write_json;
use crate::runtime::youki::volumes::AttachedDisk;
use pistack_types::ApiResult;
use std::path::Path;

pub(in crate::runtime::youki) fn write_disks(
    bundle: &Path,
    disks: &[AttachedDisk],
) -> ApiResult<()> {
    write_json(&disks_path(bundle), &disks, 0o644)
}
