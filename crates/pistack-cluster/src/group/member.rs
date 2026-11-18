// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use serde::{Deserialize, Serialize};

use super::role::Role;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Member {
    pub id: String,
    pub api: String,
    pub role: Role,
    /// Where host agents reach this server: its TLS listener, the only one
    /// that can check a client certificate. `None` for a server with no TLS
    /// listener, and in records written before the field existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_api: Option<String>,
}

impl Member {
    pub fn voter(id: &str, api: &str) -> Self {
        Self {
            id: id.into(),
            api: api.into(),
            role: Role::Voter,
            agent_api: None,
        }
    }

    pub fn learner(id: &str, api: &str) -> Self {
        Self {
            id: id.into(),
            api: api.into(),
            role: Role::Learner,
            agent_api: None,
        }
    }

    pub fn with_agent_api(mut self, agent_api: Option<&str>) -> Self {
        self.agent_api = agent_api.map(str::to_owned);
        self
    }
}
