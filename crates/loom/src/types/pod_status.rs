// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use chrono::{DateTime, Utc};
use pistack_types::{Condition, Id};
use serde::{Deserialize, Serialize};

// ---- kinds and well-known keys ------------------------------------------------

use super::container_status::ContainerStatus;
use super::pod_phase::PodPhase;
use super::pod_ready::POD_READY;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct PodStatus {
    #[serde(default)]
    pub phase: PodPhase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pod_ip: Option<String>,
    /// forge host id the backing machine runs on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<Id>,
    /// The forge machine backing this pod.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine: Option<Id>,
    #[serde(default)]
    pub container_statuses: Vec<ContainerStatus>,
    #[serde(default)]
    pub conditions: Vec<Condition>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_time: Option<DateTime<Utc>>,
}

impl PodStatus {
    /// Whether controllers should treat this pod as ready to serve.
    ///
    /// True when the [`POD_READY`] condition is present and true. A pod
    /// with no readiness probe reaches that once every container is
    /// running. A readiness probe holds the condition false until the
    /// probe has passed.
    pub fn is_ready(&self) -> bool {
        self.conditions
            .iter()
            .any(|c| c.kind == POD_READY && c.status)
    }
}
