// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

pub(crate) fn rbd_target(keyring: Option<String>) -> pistack_rpc::proto::DiskTarget {
    pistack_rpc::proto::DiskTarget::Rbd {
        pool: "volumes".into(),
        image: "img-1".into(),
        monitors: vec!["10.1.0.5:6789".into(), "mon.example".into()],
        user: "pistack".into(),
        key_secret: keyring,
    }
}
