// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_rpc::proto::DiskTarget;
use pistack_types::ApiResult;

pub(in crate::runtime::youki) async fn attach_host_path(
    req: &crate::runtime::VolumeAttach,
) -> ApiResult<(String, bool)> {
    match &req.target {
        Some(DiskTarget::Rbd {
            pool,
            image,
            monitors,
            user,
            key_secret,
        }) => {
            let path = crate::runtime::rbdmap::map_image(
                pool,
                image,
                monitors,
                user,
                key_secret,
                req.read_only,
            )
            .await?;
            Ok((path, true))
        }
        _ => Ok((req.path.clone(), false)),
    }
}
