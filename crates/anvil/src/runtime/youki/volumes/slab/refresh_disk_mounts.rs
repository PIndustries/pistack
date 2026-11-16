// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::volumes::container_dir;
use crate::runtime::youki::volumes::oci_binds;
use crate::runtime::youki::volumes::patch_mounts;
use crate::runtime::youki::volumes::read_disks;
use crate::runtime::youki::volumes::read_io;
use crate::runtime::youki::volumes::MountPlan;
use pistack_types::ApiError;
use pistack_types::ApiResult;
use std::path::Path;

pub(in crate::runtime::youki) fn refresh_disk_mounts(bundle: &Path) -> ApiResult<()> {
    let path = bundle.join("mount-plan.json");
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(read_io(&path, e)),
    };
    let plan: MountPlan = serde_json::from_slice(&bytes).map_err(|e| {
        ApiError::unavailable(format!("youki runtime: reading {}: {e}", path.display()))
    })?;
    let disks = read_disks(bundle)?;
    for container in &plan.containers {
        let binds = oci_binds(
            bundle,
            &plan.volumes,
            &container.mounts,
            &disks,
            &plan.volume_paths,
        );
        let config = container_dir(bundle, container.index).join("config.json");
        patch_mounts(&config, &binds)?;
    }
    Ok(())
}
