// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::kinds;
use pistack_types::Spec;
use serde::{Deserialize, Serialize};

// ---- kinds and well-known keys ------------------------------------------------

use super::one::one;
use super::pod_management_policy::PodManagementPolicy;
use super::pod_template::PodTemplate;
use super::selector::Selector;
use super::stateful_set_update_strategy::StatefulSetUpdateStrategy;
use super::volume_claim_template::VolumeClaimTemplate;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct StatefulSetSpec {
    #[serde(default = "one")]
    pub replicas: u32,
    /// Defaults to the template labels when empty.
    #[serde(default)]
    pub selector: Selector,
    pub template: PodTemplate,
    /// Name of the Service that gives pods a stable network identity.
    /// Empty when unused; loom does not invent DNS records here.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub service_name: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub volume_claim_templates: Vec<VolumeClaimTemplate>,
    #[serde(default)]
    pub pod_management_policy: PodManagementPolicy,
    #[serde(default)]
    pub update_strategy: StatefulSetUpdateStrategy,
}

impl Spec for StatefulSetSpec {
    const KIND: &'static str = kinds::STATEFUL_SET;
}
