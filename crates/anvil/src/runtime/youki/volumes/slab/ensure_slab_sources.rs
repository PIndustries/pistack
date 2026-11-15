// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// Every mounted slab volume resolves, or create stops.
use crate::runtime::youki::spec::PodPlan;
use crate::runtime::youki::spec::VolumeSrc;
use crate::runtime::youki::volumes::slab_prepared;
use crate::runtime::youki::volumes::valid_destination;
use crate::runtime::youki::volumes::AttachedDisk;
use pistack_types::ApiResult;

pub(in crate::runtime::youki) fn ensure_slab_sources(
    plan: &PodPlan,
    disks: &[AttachedDisk],
) -> ApiResult<()> {
    for vol in &plan.volumes {
        let VolumeSrc::Volume(id) = &vol.source else {
            continue;
        };
        let used = plan.containers.iter().any(|container| {
            container.volume_mounts.iter().any(|mount| {
                mount.name == vol.name
                    && !mount.name.is_empty()
                    && valid_destination(&mount.mount_path)
            })
        });
        if used {
            slab_prepared(vol, id, plan, disks)?;
        }
    }
    Ok(())
}
