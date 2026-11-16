// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

mod mounts;
pub(in crate::runtime::youki) use mounts::*;

mod slab;
pub(in crate::runtime::youki) use slab::*;

#[cfg(test)]
#[path = "late_attach_and_recreate_bind_the_mounted_folder_tests.rs"]
mod late_attach_and_recreate_bind_the_mounted_folder_tests;

#[cfg(test)]
#[path = "mounted_slab_folder_is_accepted_tests.rs"]
mod mounted_slab_folder_is_accepted_tests;

#[cfg(test)]
#[path = "pod_bundles_mount_every_container_tests.rs"]
mod pod_bundles_mount_every_container_tests;

#[cfg(test)]
#[path = "rbd_pod_maps_a_host_device_and_unmaps_on_detach_tests.rs"]
mod rbd_pod_maps_a_host_device_and_unmaps_on_detach_tests;

#[cfg(test)]
#[path = "rbd_sudo_maps_for_a_non_root_agent_and_delete_unmaps_tests.rs"]
mod rbd_sudo_maps_for_a_non_root_agent_and_delete_unmaps_tests;

#[cfg(test)]
#[path = "sudo_mode_opens_secret_volumes_and_keeps_the_bundle_private_tests.rs"]
mod sudo_mode_opens_secret_volumes_and_keeps_the_bundle_private_tests;
