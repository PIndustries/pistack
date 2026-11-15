// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// Binds for one container at create time. A slab volume fails when its
/// folder is missing, is not a directory, or is not a mount point. Other
/// volumes keep the lenient mount plan.
use crate::runtime::youki::spec::PodPlan;
use crate::runtime::youki::spec::SandboxMount;
use crate::runtime::youki::spec::VolumeSrc;
use crate::runtime::youki::volumes::mount_source;
use crate::runtime::youki::volumes::prepared_volume;
use crate::runtime::youki::volumes::slab_prepared;
use crate::runtime::youki::volumes::valid_destination;
use crate::runtime::youki::volumes::AttachedDisk;
use crate::runtime::youki::volumes::OciBind;
use pistack_types::ApiResult;
use std::path::Path;

pub(in crate::runtime::youki) fn pod_binds(
    pod: &Path,
    plan: &PodPlan,
    mounts: &[SandboxMount],
    disks: &[AttachedDisk],
) -> ApiResult<Vec<OciBind>> {
    let mut out = Vec::new();
    for mount in mounts {
        if mount.name.is_empty() || !valid_destination(&mount.mount_path) {
            continue;
        }
        let Some(index) = plan.volumes.iter().position(|v| v.name == mount.name) else {
            continue;
        };
        let vol = &plan.volumes[index];
        let prepared = match &vol.source {
            VolumeSrc::Volume(id) => match slab_prepared(vol, id, plan, disks)? {
                Some(prepared) => prepared,
                None => continue,
            },
            _ => match prepared_volume(pod, index, vol, disks, &plan.volume_paths) {
                Some(prepared) => prepared,
                None => continue,
            },
        };
        let Some((source, directory)) = mount_source(&prepared, &mount.sub_path) else {
            continue;
        };
        out.push(OciBind {
            destination: mount.mount_path.clone(),
            source,
            read_only: prepared.force_read_only || mount.read_only,
            directory,
        });
    }
    Ok(out)
}
