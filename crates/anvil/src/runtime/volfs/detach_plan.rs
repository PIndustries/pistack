// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::argv::argv;
use super::losetup_detach_args::losetup_detach_args;
use super::umount_args::umount_args;

pub(in crate::runtime::volfs) fn detach_plan(
    mount_dir: &str,
    loop_device: Option<&str>,
) -> Vec<Vec<String>> {
    let mut steps = vec![argv("umount", &umount_args(mount_dir))];
    if let Some(device) = loop_device {
        steps.push(argv("losetup", &losetup_detach_args(device)));
    }
    steps
}
