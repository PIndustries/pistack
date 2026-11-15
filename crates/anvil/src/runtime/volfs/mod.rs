// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Mount a slab volume as a directory for a pod.
//!
//! A pod attach turns the block device into a filesystem and mounts it at
//! `$PISTACK_DATA_DIR/slab/mounts/<volume-id>` (or `$PISTACK_VOLFS_DATA_DIR`
//! when that is set). The agent records that directory as the host path.
//! A VM attach does not come here.
//!
//! A local image file is attached with `losetup --find --show`. An LVM
//! device node and an RBD device mapped by `rbdmap` are used as they are.
//! `blkid -p -o value -s TYPE` probes the filesystem. Empty output formats
//! with `mkfs.ext4 -q -F`, or `mkfs.<fs_type>` when the volume type sets
//! `fs_type`. A device that already has a filesystem is never formatted.
//!
//! Detach unmounts that directory, then `losetup -d` when a loop device
//! was used. An unmount that reports the directory is not mounted is
//! success. The caller then runs the existing LVM or RBD detach.
//!
//! A root agent runs the tools directly. `PISTACK_VOLFS_SUDO` does not
//! change that. When the agent is not root and `PISTACK_VOLFS_SUDO` is
//! `1`, `true`, or `yes`, `losetup`, `blkid`, `mkfs`, `mount`, and `umount`
//! run as `sudo -n`. `sudo -n` fails closed without a rule. Allow only
//! those binaries:
//!
//! ```text
//! # /etc/sudoers.d/pistack-volfs
//! ubuntu ALL=(root) NOPASSWD: /sbin/losetup, /sbin/blkid, /sbin/mkfs.ext4, /bin/mount, /bin/umount
//! ```
//!
//! Replace `ubuntu` and the paths with the agent user and the binaries on
//! the host. A volume type that sets `fs_type` to something other than
//! `ext4` needs that `mkfs` binary on the same rule.
//! `$PISTACK_LOSETUP`, `$PISTACK_BLKID`, `$PISTACK_MKFS`, `$PISTACK_MOUNT`,
//! and `$PISTACK_UMOUNT` replace those binaries. `$PISTACK_VOLFS_SUDO_BIN`
//! replaces `sudo` for tests.

mod agent_is_root;
mod already_detached;
mod already_unmounted;
#[cfg(test)]
mod argv;
#[cfg(test)]
mod attach_plan;
mod blkid_args;
mod block_source;
mod brief;
mod canonical_fs;
mod clip;
mod combined;
mod command_failed;
mod command_path_ok;
mod consts;
mod data_dir;
mod detach_loop_idempotent;
#[cfg(test)]
mod detach_plan;
mod disk_kind;
mod env_flag;
#[cfg(test)]
mod env_lock;
#[cfg(test)]
pub(crate) use env_lock::env_lock;
mod exec;
mod executable;
mod filesystem_type;
mod first_token;
mod io_err;
mod is_block_device;
mod is_regular_file;
mod loop_device_ok;
mod losetup_detach_args;
mod losetup_find;
mod losetup_find_args;
mod make_filesystem;
mod mkfs_command;
mod mount_args;
mod mount_device;
mod mount_for_pod;
pub(crate) use mount_for_pod::mount_for_pod;
#[cfg(test)]
mod mount_plan;
mod prepare_mount_dir;
mod privilege_hint;
mod read_sidecar;
mod release_volume;
pub(crate) use release_volume::release_volume;
mod rollback;
mod sudo_program;
mod tool_program;
mod umount_args;
mod unmount_idempotent;
mod volume_name;
mod volume_paths;
mod want_sudo;
mod write_sidecar;

#[cfg(test)]
mod blkid_type_skips_mkfs_for_file_lvm_and_rbd_tests;
#[cfg(test)]
mod command_plan_for_each_backend_tests;
#[cfg(test)]
mod detach_is_ordered_and_idempotent_when_already_unmounted_tests;
#[cfg(test)]
mod detach_plan_unmounts_then_detaches_a_loop_tests;
#[cfg(test)]
mod existing_filesystem_is_not_in_the_plan_tests;
#[cfg(test)]
mod filesystem_type_rejects_a_command_string_tests;
#[cfg(test)]
mod local_file_formats_and_records_the_mount_directory_tests;
#[cfg(test)]
mod mkfs_failure_does_not_mount_and_detaches_the_loop_tests;
#[cfg(test)]
mod mount_failure_rolls_back_the_loop_device_tests;
#[cfg(test)]
mod non_root_agent_runs_the_tools_through_sudo_tests;
#[cfg(test)]
pub(in crate::runtime::volfs) mod test_support;
