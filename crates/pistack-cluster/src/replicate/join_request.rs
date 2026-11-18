// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JoinRequest {
    pub node_id: String,
    pub api: String,
    /// The joiner's address for host agents, when it has a TLS listener.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_api: Option<String>,
}
