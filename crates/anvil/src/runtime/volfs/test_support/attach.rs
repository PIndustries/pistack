// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::VolumeAttach;
use pistack_rpc::proto::DiskTarget;
use pistack_types::kind;
use pistack_types::Id;

pub(in crate::runtime::volfs) fn attach(
    path: &str,
    fs: &str,
    read_only: bool,
    rbd: bool,
) -> VolumeAttach {
    VolumeAttach {
        machine: Id::new(kind::MACHINE),
        volume: Id::new(kind::VOLUME),
        path: path.to_owned(),
        device: Some("vdb".into()),
        read_only,
        target: rbd.then(|| DiskTarget::Rbd {
            pool: "volumes".into(),
            image: "img-1".into(),
            monitors: Vec::new(),
            user: "pistack".into(),
            key_secret: None,
        }),
        fs_type: fs.to_owned(),
    }
}
