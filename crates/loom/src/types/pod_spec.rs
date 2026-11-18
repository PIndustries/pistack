// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::kinds;
use pistack_types::{Resources, Spec};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

// ---- kinds and well-known keys ------------------------------------------------

use super::container::Container;
use super::pod_volume::PodVolume;
use super::restart_policy::RestartPolicy;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct PodSpec {
    pub containers: Vec<Container>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub volumes: Vec<PodVolume>,
    #[serde(default)]
    pub restart_policy: RestartPolicy,
    /// Every pair must match a host label (forge `host_selector`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub node_selector: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub service_account: String,
    /// fabric networks to attach. Empty means `default` if the project has one,
    /// else the project's only non-external network.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub networks: Vec<String>,
}

impl Spec for PodSpec {
    const KIND: &'static str = kinds::POD;
}

impl PodSpec {
    /// Sum of the containers' requests: what the pod's machine must offer.
    pub fn resources(&self) -> Resources {
        self.containers
            .iter()
            .fold(Resources::default(), |acc, c| acc.add(&c.resources))
    }
}
