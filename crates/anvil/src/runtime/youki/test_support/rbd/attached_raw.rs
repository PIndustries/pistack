// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::volumes::AttachedDisk;

pub(crate) fn attached_raw(vol_id: &str, path: &str, read_only: bool) -> AttachedDisk {
    AttachedDisk {
        volume: vol_id.into(),
        path: path.into(),
        device: "vdb".into(),
        read_only,
        rbd_mapped: true,
    }
}
