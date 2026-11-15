// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::spec::SandboxVolume;
use crate::runtime::youki::volumes::PlannedMounts;
use serde::Deserialize;
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Clone, Serialize, Deserialize)]
pub struct MountPlan {
    pub(in crate::runtime::youki) volumes: Vec<SandboxVolume>,
    pub(in crate::runtime::youki) containers: Vec<PlannedMounts>,
    /// Mounted directories for slab volumes. Plans written before this field
    /// existed omit it; those volumes are not bound from the raw disk path.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(in crate::runtime::youki) volume_paths: BTreeMap<String, String>,
}
