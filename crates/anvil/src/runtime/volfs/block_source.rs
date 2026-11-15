// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::disk_kind::DiskKind;
use crate::runtime::VolumeAttach;
use pistack_rpc::proto::DiskTarget;

pub(in crate::runtime::volfs) fn block_source<'a>(
    req: &'a VolumeAttach,
    recorded: &'a str,
    kind: DiskKind,
) -> &'a str {
    if kind == DiskKind::Device && matches!(req.target, Some(DiskTarget::Rbd { .. })) {
        recorded
    } else {
        req.path.as_str()
    }
}
