// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::path::PathBuf;

pub(in crate::runtime::volfs) struct Tools {
    pub(in crate::runtime::volfs) log: PathBuf,
    pub(in crate::runtime::volfs) empty: PathBuf,
    pub(in crate::runtime::volfs) mkfs_fail: PathBuf,
    pub(in crate::runtime::volfs) mount_fail: PathBuf,
    pub(in crate::runtime::volfs) not_mounted: PathBuf,
    pub(in crate::runtime::volfs) losetup: PathBuf,
    pub(in crate::runtime::volfs) blkid: PathBuf,
    pub(in crate::runtime::volfs) mkfs: PathBuf,
    pub(in crate::runtime::volfs) mount: PathBuf,
    pub(in crate::runtime::volfs) umount: PathBuf,
    pub(in crate::runtime::volfs) sudo: PathBuf,
    pub(in crate::runtime::volfs) sudo_log: PathBuf,
}
