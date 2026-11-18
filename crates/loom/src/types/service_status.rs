// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::Condition;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

// ---- kinds and well-known keys ------------------------------------------------

use super::endpoint::Endpoint;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ServiceStatus {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cluster_ip: Option<String>,
    #[serde(default)]
    pub endpoints: Vec<Endpoint>,
    /// External addresses for `LoadBalancer` services, filled by the realizer.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub load_balancer: Vec<String>,
    /// Node port held for each service port, keyed by `port`. Set for
    /// `NodePort` and `LoadBalancer` services: `spec` asks for one, or the
    /// controller takes the lowest free port in 30000-32767.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub node_ports: BTreeMap<u16, u16>,
    #[serde(default)]
    pub conditions: Vec<Condition>,
}
