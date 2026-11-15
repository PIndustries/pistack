// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::Id;

pub(crate) fn rbd_attach(
    machine: Id,
    volume: Id,
    target: pistack_rpc::proto::DiskTarget,
) -> crate::runtime::VolumeAttach {
    crate::runtime::VolumeAttach {
        machine,
        volume,
        path: String::new(),
        device: None,
        read_only: true,
        fs_type: String::new(),
        target: Some(target),
    }
}
