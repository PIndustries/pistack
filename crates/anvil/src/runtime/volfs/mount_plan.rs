// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::disk_kind::DiskKind;

/// Where a pod mount is planned, before any command runs.
pub(in crate::runtime::volfs) struct MountPlan<'a> {
    pub(in crate::runtime::volfs) kind: DiskKind,
    pub(in crate::runtime::volfs) source: &'a str,
    /// Device `losetup --find --show` printed. Ignored for [`DiskKind::Device`].
    pub(in crate::runtime::volfs) loop_device: &'a str,
    pub(in crate::runtime::volfs) mount_dir: &'a str,
    pub(in crate::runtime::volfs) fs_type: &'a str,
    pub(in crate::runtime::volfs) has_filesystem: bool,
    pub(in crate::runtime::volfs) read_only: bool,
}
