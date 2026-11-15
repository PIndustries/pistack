// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::volumes::read_disks;
use pistack_types::ApiResult;
use std::path::Path;

pub(in crate::runtime::youki) async fn unmap_recorded(bundle: &Path) -> ApiResult<()> {
    for disk in read_disks(bundle)? {
        if disk.rbd_mapped {
            crate::runtime::rbdmap::unmap_device(&disk.path).await?;
        }
    }
    Ok(())
}
