// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use pistack_types::Resources;
use serde::{Deserialize, Serialize};

use crate::proto::MachineReport;

/// What the control plane knows about one host. A snapshot; cheap to clone.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct NodeInfo {
    pub name: String,
    pub capabilities: Vec<String>,
    pub labels: BTreeMap<String, String>,
    pub version: String,
    /// Underlay IPv4 from the last hello. None when the agent left it empty.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub underlay_ip: Option<String>,
    pub resources_total: Resources,
    pub resources_free: Resources,
    pub machines: Vec<MachineReport>,
    pub last_hello: Option<DateTime<Utc>>,
    pub last_heartbeat: Option<DateTime<Utc>>,
    /// Commands delivered to the agent and not yet answered.
    pub in_flight: usize,
    /// Commands queued and not yet fetched.
    pub queued: usize,
}
