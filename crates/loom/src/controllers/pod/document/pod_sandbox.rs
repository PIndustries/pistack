// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::controllers::pod::document::{PodIdentity, ProjectedFiles};
use crate::types::RestartPolicy;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
/// The document a pod's machine carries in `spec.user_data`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PodSandbox {
    /// Always `"loom.pistack.io/v1"`.
    pub api_version: String,
    /// Always `"PodSandbox"`.
    pub kind: String,
    pub pod: PodIdentity,
    pub restart_policy: RestartPolicy,
    pub containers: Vec<crate::types::Container>,
    pub volumes: Vec<crate::types::PodVolume>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub service_account: String,
    pub networks: Vec<String>,
    /// ConfigMap file bodies keyed by name and by id. Empty from
    /// [`PodSandbox::from_pod`]; filled when the machine is created.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub config_maps: BTreeMap<String, ProjectedFiles>,
    /// Secret file bodies keyed by name and by id. Same lifetime as
    /// [`Self::config_maps`]. Payloads are not part of the pod spec.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub secrets: BTreeMap<String, ProjectedFiles>,
    /// Slab volume id to the mounted directory recorded at attach.
    /// Empty until the volume is attached. The youki runtime bind-mounts
    /// this directory. It does not bind the volume's backing file.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub volume_paths: BTreeMap<String, String>,
    /// Slab volume id to a device name, when the volume has no mounted
    /// directory yet. The runtime does not bind that device.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub volume_devices: BTreeMap<String, String>,
}
