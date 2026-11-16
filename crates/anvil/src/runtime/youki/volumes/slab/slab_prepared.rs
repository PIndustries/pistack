// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::spec::PodPlan;
use crate::runtime::youki::spec::SandboxVolume;
use crate::runtime::youki::volumes::raw_source_error;
use crate::runtime::youki::volumes::require_mounted_dir;
use crate::runtime::youki::volumes::volume_label;
use crate::runtime::youki::volumes::AttachedDisk;
use crate::runtime::youki::volumes::PreparedVolume;
use pistack_types::ApiError;
use pistack_types::ApiResult;

pub(in crate::runtime::youki) fn slab_prepared(
    vol: &SandboxVolume,
    id: &str,
    plan: &PodPlan,
    disks: &[AttachedDisk],
) -> ApiResult<Option<PreparedVolume>> {
    let disk = disks.iter().find(|d| d.volume == id);
    let label = volume_label(vol, id);
    // The recorded host path is the mounted folder. `disks.json` keeps the
    // backing file or mapped device for detach and is not a bind source.
    if let Some(path) = plan
        .volume_paths
        .get(id)
        .map(String::as_str)
        .filter(|path| !path.is_empty())
    {
        let host_path = require_mounted_dir(label, path)?;
        return Ok(Some(PreparedVolume {
            host_path,
            directory: true,
            force_read_only: disk.is_some_and(|d| d.read_only),
        }));
    }
    if let Some(disk) = disk.filter(|d| !d.path.is_empty()) {
        return Err(raw_source_error(label, &disk.path));
    }
    // A device name and no mounted directory yet. Mapping may store the
    // device after the bundle exists; create does not bind it.
    if plan.volume_devices.contains_key(id) {
        return Ok(None);
    }
    Err(ApiError::unavailable(format!(
        "youki runtime: slab volume {label} has no host path"
    )))
}
