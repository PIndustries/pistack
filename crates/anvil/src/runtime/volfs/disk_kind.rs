// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::is_block_device::is_block_device;
use super::is_regular_file::is_regular_file;
use crate::runtime::VolumeAttach;
use pistack_rpc::proto::DiskTarget;
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::runtime::volfs) enum DiskKind {
    /// Local image file. `losetup` yields the block device.
    File,
    /// LVM device node or mapped RBD device.
    Device,
}

pub(in crate::runtime::volfs) fn disk_kind(req: &VolumeAttach, recorded: &str) -> DiskKind {
    if matches!(req.target, Some(DiskTarget::Rbd { .. })) && !recorded.is_empty() {
        return DiskKind::Device;
    }
    let path = Path::new(&req.path);
    if is_block_device(path) {
        return DiskKind::Device;
    }
    if req.path.starts_with("/dev/") && !is_regular_file(path) {
        return DiskKind::Device;
    }
    DiskKind::File
}
