// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::spec::Projected;
use crate::runtime::youki::spec::SandboxContainer;
use crate::runtime::youki::spec::SandboxVolume;
use serde::Deserialize;
use std::collections::BTreeMap;

#[derive(Clone, Debug, Deserialize)]
pub struct SandboxDoc {
    #[serde(default)]
    pub(in crate::runtime::youki) kind: String,
    #[serde(default)]
    pub(in crate::runtime::youki) containers: Vec<SandboxContainer>,
    #[serde(default)]
    pub(in crate::runtime::youki) volumes: Vec<SandboxVolume>,
    #[serde(default)]
    pub(in crate::runtime::youki) config_maps: BTreeMap<String, Projected>,
    #[serde(default)]
    pub(in crate::runtime::youki) secrets: BTreeMap<String, Projected>,
    /// Slab volume id to the mounted directory recorded at attach. Loom
    /// writes this once the volume is attached. This directory is the
    /// container bind. The raw file or device in `disks.json` is not.
    #[serde(default)]
    pub(in crate::runtime::youki) volume_paths: BTreeMap<String, String>,
    /// Slab volume id to a device name, when the volume has no mounted
    /// directory yet. Create does not bind that device.
    #[serde(default)]
    pub(in crate::runtime::youki) volume_devices: BTreeMap<String, String>,
}
