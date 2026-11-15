// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::time::Duration;

pub(in crate::runtime::volfs) const PROBE_TIMEOUT: Duration = Duration::from_secs(30);
pub(in crate::runtime::volfs) const MKFS_TIMEOUT: Duration = Duration::from_secs(120);
pub(in crate::runtime::volfs) const LOSETUP_ENV: &str = "PISTACK_LOSETUP";
pub(in crate::runtime::volfs) const BLKID_ENV: &str = "PISTACK_BLKID";
pub(in crate::runtime::volfs) const MKFS_ENV: &str = "PISTACK_MKFS";
pub(in crate::runtime::volfs) const MOUNT_ENV: &str = "PISTACK_MOUNT";
pub(in crate::runtime::volfs) const UMOUNT_ENV: &str = "PISTACK_UMOUNT";
pub(in crate::runtime::volfs) const SUDO_ENV: &str = "PISTACK_VOLFS_SUDO";
pub(in crate::runtime::volfs) const SUDO_BIN_ENV: &str = "PISTACK_VOLFS_SUDO_BIN";
pub(in crate::runtime::volfs) const DATA_ENV: &str = "PISTACK_VOLFS_DATA_DIR";
pub(in crate::runtime::volfs) const SUDOERS_SAMPLE: &str = "ubuntu ALL=(root) NOPASSWD: /sbin/losetup, /sbin/blkid, /sbin/mkfs.ext4, /bin/mount, /bin/umount";
