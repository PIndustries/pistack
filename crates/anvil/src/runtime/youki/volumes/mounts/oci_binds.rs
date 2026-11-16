// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::spec::SandboxMount;
use crate::runtime::youki::spec::SandboxVolume;
use crate::runtime::youki::volumes::mount_source;
use crate::runtime::youki::volumes::prepared_volume;
use crate::runtime::youki::volumes::valid_destination;
use crate::runtime::youki::volumes::AttachedDisk;
use crate::runtime::youki::volumes::OciBind;
use std::collections::BTreeMap;
use std::path::Path;

pub(in crate::runtime::youki) fn oci_binds(
    pod: &Path,
    volumes: &[SandboxVolume],
    mounts: &[SandboxMount],
    disks: &[AttachedDisk],
    slab_paths: &BTreeMap<String, String>,
) -> Vec<OciBind> {
    let mut out = Vec::new();
    for mount in mounts {
        if mount.name.is_empty() || !valid_destination(&mount.mount_path) {
            continue;
        }
        let Some(index) = volumes.iter().position(|v| v.name == mount.name) else {
            continue;
        };
        let Some(prepared) = prepared_volume(pod, index, &volumes[index], disks, slab_paths) else {
            continue;
        };
        let Some((source, directory)) = mount_source(&prepared, &mount.sub_path) else {
            continue;
        };
        out.push(OciBind {
            destination: mount.mount_path.clone(),
            source,
            read_only: prepared.force_read_only || mount.read_only,
            directory,
        });
    }
    out
}
