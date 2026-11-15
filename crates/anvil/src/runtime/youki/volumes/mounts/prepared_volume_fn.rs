// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::spec::SandboxVolume;
use crate::runtime::youki::spec::VolumeSrc;
use crate::runtime::youki::volumes::folder_is_mount_point;
use crate::runtime::youki::volumes::volume_host_dir;
use crate::runtime::youki::volumes::AttachedDisk;
use crate::runtime::youki::volumes::PreparedVolume;
use std::collections::BTreeMap;
use std::path::Path;
use std::path::PathBuf;

pub(in crate::runtime::youki) fn prepared_volume(
    pod: &Path,
    index: usize,
    vol: &SandboxVolume,
    disks: &[AttachedDisk],
    slab_paths: &BTreeMap<String, String>,
) -> Option<PreparedVolume> {
    match &vol.source {
        VolumeSrc::EmptyDir | VolumeSrc::ConfigMap(_) | VolumeSrc::Secret(_) => {
            Some(PreparedVolume {
                host_path: volume_host_dir(pod, index, &vol.name),
                directory: true,
                force_read_only: false,
            })
        }
        // The bind is the mounted folder. A raw file or device in
        // `disks.json` is not a source. Attach rewrites this config before
        // the folder is recorded, so a missing path, a non-directory, or a
        // directory that is not a mount point is left unbound instead of
        // failing the attach.
        VolumeSrc::Volume(id) => {
            let disk = disks.iter().find(|d| &d.volume == id)?;
            let path = slab_paths.get(id).filter(|path| !path.is_empty())?;
            let host_path = PathBuf::from(path);
            if !host_path.is_dir() || !matches!(folder_is_mount_point(path), Ok(true)) {
                return None;
            }
            Some(PreparedVolume {
                host_path,
                directory: true,
                force_read_only: disk.read_only,
            })
        }
    }
}
