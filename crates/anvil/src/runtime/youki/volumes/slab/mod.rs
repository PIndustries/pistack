// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

#[path = "attach_host_path.rs"]
mod attach_host_path;
pub(in crate::runtime::youki) use attach_host_path::*;

#[cfg(test)]
#[path = "attach_is_not_found_until_the_bundle_exists_tests.rs"]
mod attach_is_not_found_until_the_bundle_exists_tests;

#[path = "check_device_name.rs"]
mod check_device_name;
pub(in crate::runtime::youki) use check_device_name::*;

#[path = "classify_slab_path.rs"]
mod classify_slab_path;
pub(in crate::runtime::youki) use classify_slab_path::*;

#[cfg(test)]
#[path = "device_names_and_host_paths_tests.rs"]
mod device_names_and_host_paths_tests;

#[path = "disks_path.rs"]
mod disks_path;
pub(in crate::runtime::youki) use disks_path::*;

#[path = "ensure_slab_sources.rs"]
mod ensure_slab_sources;
pub(in crate::runtime::youki) use ensure_slab_sources::*;

#[path = "folder_is_mount_point.rs"]
mod folder_is_mount_point;
pub(in crate::runtime::youki) use folder_is_mount_point::*;

#[path = "host_path_ok.rs"]
mod host_path_ok;
pub(in crate::runtime::youki) use host_path_ok::*;

#[path = "is_block_device.rs"]
mod is_block_device;
pub(in crate::runtime::youki) use is_block_device::*;

#[path = "mounted_by_device.rs"]
mod mounted_by_device;
pub(in crate::runtime::youki) use mounted_by_device::*;

#[path = "mountinfo_file.rs"]
mod mountinfo_file;
pub(in crate::runtime::youki) use mountinfo_file::*;

#[path = "mountinfo_lists.rs"]
mod mountinfo_lists;
pub(in crate::runtime::youki) use mountinfo_lists::*;

#[cfg(test)]
#[path = "mountinfo_matches_the_exact_path_and_unescapes_spaces_tests.rs"]
mod mountinfo_matches_the_exact_path_and_unescapes_spaces_tests;

#[path = "mountinfo_point.rs"]
mod mountinfo_point;
pub(in crate::runtime::youki) use mountinfo_point::*;

#[path = "raw_source_error.rs"]
mod raw_source_error;
pub(in crate::runtime::youki) use raw_source_error::*;

#[path = "read_disks.rs"]
mod read_disks;
pub(in crate::runtime::youki) use read_disks::*;

#[path = "refresh_disk_mounts.rs"]
mod refresh_disk_mounts;
pub(in crate::runtime::youki) use refresh_disk_mounts::*;

#[path = "release_rbd.rs"]
mod release_rbd;
pub(in crate::runtime::youki) use release_rbd::*;

#[path = "require_mounted_dir.rs"]
mod require_mounted_dir;
pub(in crate::runtime::youki) use require_mounted_dir::*;

#[cfg(test)]
#[path = "slab_bind_source_must_exist_before_create_tests.rs"]
mod slab_bind_source_must_exist_before_create_tests;

#[cfg(test)]
#[path = "slab_mount_binds_the_recorded_folder_tests.rs"]
mod slab_mount_binds_the_recorded_folder_tests;

#[path = "slab_prepared.rs"]
mod slab_prepared;
pub(in crate::runtime::youki) use slab_prepared::*;

#[path = "slab_source.rs"]
mod slab_source;
pub(in crate::runtime::youki) use slab_source::*;

#[path = "slab_source_error.rs"]
mod slab_source_error;
pub(in crate::runtime::youki) use slab_source_error::*;

#[cfg(test)]
#[path = "slab_source_errors_name_the_volume_tests.rs"]
mod slab_source_errors_name_the_volume_tests;

#[path = "slab_source_io.rs"]
mod slab_source_io;
pub(in crate::runtime::youki) use slab_source_io::*;

#[path = "unescape_mount.rs"]
mod unescape_mount;
pub(in crate::runtime::youki) use unescape_mount::*;

#[path = "unmap_recorded.rs"]
mod unmap_recorded;
pub(in crate::runtime::youki) use unmap_recorded::*;

#[cfg(test)]
#[path = "unmounted_slab_folder_is_refused_tests.rs"]
mod unmounted_slab_folder_is_refused_tests;

#[path = "write_disks.rs"]
mod write_disks;
pub(in crate::runtime::youki) use write_disks::*;
