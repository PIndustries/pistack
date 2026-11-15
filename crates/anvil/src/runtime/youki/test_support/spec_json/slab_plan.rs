// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::spec::PodPlan;
use crate::runtime::youki::spec::SandboxContainer;
use crate::runtime::youki::spec::SandboxMount;
use crate::runtime::youki::spec::SandboxVolume;
use crate::runtime::youki::spec::VolumeSrc;
use std::collections::BTreeMap;

pub(crate) fn slab_plan(
    image: &str,
    vol_id: &str,
    path: Option<&str>,
    device: Option<&str>,
) -> PodPlan {
    let mut volume_paths = BTreeMap::new();
    if let Some(path) = path {
        volume_paths.insert(vol_id.to_string(), path.to_string());
    }
    let mut volume_devices = BTreeMap::new();
    if let Some(device) = device {
        volume_devices.insert(vol_id.to_string(), device.to_string());
    }
    PodPlan {
        containers: vec![SandboxContainer {
            name: "web".into(),
            image: image.into(),
            volume_mounts: vec![SandboxMount {
                name: "disk".into(),
                mount_path: "/data".into(),
                read_only: false,
                sub_path: String::new(),
            }],
            ..SandboxContainer::default()
        }],
        volumes: vec![SandboxVolume {
            name: "disk".into(),
            source: VolumeSrc::Volume(vol_id.into()),
        }],
        config_maps: BTreeMap::new(),
        secrets: BTreeMap::new(),
        volume_paths,
        volume_devices,
    }
}
