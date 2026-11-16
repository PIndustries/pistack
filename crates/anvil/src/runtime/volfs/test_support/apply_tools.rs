// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::guard::EnvGuard;
use super::tools::Tools;
use crate::runtime::volfs::consts::{
    BLKID_ENV, DATA_ENV, LOSETUP_ENV, MKFS_ENV, MOUNT_ENV, SUDO_ENV, UMOUNT_ENV,
};
use std::path::Path;

pub(in crate::runtime::volfs) fn apply_tools(tools: &Tools, data: &Path) -> EnvGuard {
    std::env::set_var("PISTACK_VOLFS_TEST_EUID", "0");
    std::env::remove_var(SUDO_ENV);
    EnvGuard::set(&[
        (LOSETUP_ENV, &tools.losetup),
        (BLKID_ENV, &tools.blkid),
        (MKFS_ENV, &tools.mkfs),
        (MOUNT_ENV, &tools.mount),
        (UMOUNT_ENV, &tools.umount),
        (DATA_ENV, data),
    ])
}
