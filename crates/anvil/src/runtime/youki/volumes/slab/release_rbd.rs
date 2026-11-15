// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::volumes::AttachedDisk;
use pistack_rpc::proto::DiskTarget;
use pistack_types::ApiResult;

pub(in crate::runtime::youki) async fn release_rbd(
    req: &crate::runtime::VolumeDetach,
    recorded: Option<&AttachedDisk>,
) -> ApiResult<()> {
    match &req.target {
        Some(DiskTarget::Rbd { pool, image, .. }) => {
            crate::runtime::rbdmap::unmap_image(pool, image).await
        }
        _ => match recorded {
            Some(disk) if disk.rbd_mapped => crate::runtime::rbdmap::unmap_device(&disk.path).await,
            _ => Ok(()),
        },
    }
}
