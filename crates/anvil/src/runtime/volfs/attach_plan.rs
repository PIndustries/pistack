// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::argv::argv;
use super::blkid_args::blkid_args;
use super::disk_kind::DiskKind;
use super::losetup_find_args::losetup_find_args;
use super::mkfs_command::mkfs_command;
use super::mount_args::mount_args;
use super::mount_plan::MountPlan;
use pistack_types::ApiResult;

pub(in crate::runtime::volfs) fn attach_plan(plan: &MountPlan<'_>) -> ApiResult<Vec<Vec<String>>> {
    let mut steps = Vec::new();
    let device = if plan.kind == DiskKind::File {
        steps.push(argv("losetup", &losetup_find_args(plan.source)));
        plan.loop_device
    } else {
        plan.source
    };
    steps.push(argv("blkid", &blkid_args(device)));
    if !plan.has_filesystem && !plan.read_only {
        let (bin, args) = mkfs_command(plan.fs_type, device)?;
        steps.push(argv(&bin, &args));
    }
    steps.push(argv(
        "mount",
        &mount_args(device, plan.mount_dir, plan.read_only),
    ));
    Ok(steps)
}
