// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::Resources;
use serde::{Deserialize, Serialize};

// ---- kinds and well-known keys ------------------------------------------------

use super::container_port::ContainerPort;
use super::env_var::EnvVar;
use super::probe::Probe;
use super::volume_mount::VolumeMount;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct Container {
    pub name: String,
    pub image: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub command: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub env: Vec<EnvVar>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ports: Vec<ContainerPort>,
    /// Requested resources; the pod's size is the sum over its containers.
    #[serde(default)]
    pub resources: Resources,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub volume_mounts: Vec<VolumeMount>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub working_dir: String,
    /// Checked on the host that runs the pod. Failure past
    /// `failure_threshold` restarts the container when the restart policy
    /// allows it. An `exec` command runs inside the container.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub liveness_probe: Option<Probe>,
    /// While this probe has not passed, the container is not ready and the
    /// pod's Ready condition stays false. Absent means the container is
    /// ready once it is running. `exec`, `http_get`, and `tcp_socket` use
    /// the same success streak.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub readiness_probe: Option<Probe>,
}
