// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::spec::PodPlan;
use crate::runtime::youki::volumes::write_json;
use crate::runtime::youki::volumes::MountPlan;
use crate::runtime::youki::volumes::PlannedMounts;
use pistack_types::ApiResult;
use std::path::Path;

pub(in crate::runtime::youki) fn write_mount_plan(pod: &Path, plan: &PodPlan) -> ApiResult<()> {
    let planned = MountPlan {
        volumes: plan.volumes.clone(),
        containers: plan
            .containers
            .iter()
            .enumerate()
            .map(|(index, container)| PlannedMounts {
                index,
                mounts: container.volume_mounts.clone(),
            })
            .collect(),
        volume_paths: plan.volume_paths.clone(),
    };
    write_json(&pod.join("mount-plan.json"), &planned, 0o644)
}
