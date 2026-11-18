// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::direction::Direction;
use super::ether_type::EtherType;
use super::port_range::PortRange;
use super::protocol::Protocol;
use super::remote::Remote;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Rule {
    /// Stable handle for `DELETE /security-groups/{id}/rules/{rule}`.
    #[serde(default = "Uuid::now_v7")]
    pub id: Uuid,
    pub direction: Direction,
    #[serde(default)]
    pub ethertype: EtherType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol: Option<Protocol>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port_range: Option<PortRange>,
    #[serde(default)]
    pub remote: Remote,
}

impl Rule {
    pub fn new(direction: Direction, ethertype: EtherType) -> Self {
        Self {
            id: Uuid::now_v7(),
            direction,
            ethertype,
            protocol: None,
            port_range: None,
            remote: Remote::Any,
        }
    }
    pub fn protocol(mut self, p: Protocol) -> Self {
        self.protocol = Some(p);
        self
    }
    pub fn ports(mut self, min: u16, max: u16) -> Self {
        self.port_range = Some(PortRange { min, max });
        self
    }
    pub fn remote(mut self, r: Remote) -> Self {
        self.remote = r;
        self
    }
}
