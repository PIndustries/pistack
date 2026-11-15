// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::spec::Projected;
use crate::runtime::youki::spec::SandboxContainer;
use crate::runtime::youki::spec::SandboxVolume;
use std::collections::BTreeMap;

#[derive(Clone, Debug)]
pub struct PodPlan {
    pub(in crate::runtime::youki) containers: Vec<SandboxContainer>,
    pub(in crate::runtime::youki) volumes: Vec<SandboxVolume>,
    pub(in crate::runtime::youki) config_maps: BTreeMap<String, Projected>,
    pub(in crate::runtime::youki) secrets: BTreeMap<String, Projected>,
    pub(in crate::runtime::youki) volume_paths: BTreeMap<String, String>,
    pub(in crate::runtime::youki) volume_devices: BTreeMap<String, String>,
}
