// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::kinds;
use pistack_types::Spec;
use serde::{Deserialize, Serialize};

// ---- kinds and well-known keys ------------------------------------------------

use super::deployment_strategy::DeploymentStrategy;
use super::one::one;
use super::pod_template::PodTemplate;
use super::selector::Selector;
use super::three::three;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeploymentSpec {
    #[serde(default = "one")]
    pub replicas: u32,
    /// Defaults to the template labels when empty.
    #[serde(default)]
    pub selector: Selector,
    pub template: PodTemplate,
    #[serde(default)]
    pub strategy: DeploymentStrategy,
    /// Old ReplicaSets to keep for rollback.
    #[serde(default = "three")]
    pub revision_history_limit: u32,
    #[serde(default)]
    pub paused: bool,
}

impl Spec for DeploymentSpec {
    const KIND: &'static str = kinds::DEPLOYMENT;
}
