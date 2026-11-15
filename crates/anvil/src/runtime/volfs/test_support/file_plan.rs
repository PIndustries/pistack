// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::volfs::disk_kind::DiskKind;
use crate::runtime::volfs::mount_plan::MountPlan;

pub(in crate::runtime::volfs) fn file_plan<'a>(
    source: &'a str,
    loop_device: &'a str,
    mount_dir: &'a str,
    fs: &'a str,
    has_filesystem: bool,
    read_only: bool,
) -> MountPlan<'a> {
    MountPlan {
        kind: DiskKind::File,
        source,
        loop_device,
        mount_dir,
        fs_type: fs,
        has_filesystem,
        read_only,
    }
}
