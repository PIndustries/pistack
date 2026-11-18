// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::{kind, Id};
use serde::{Deserialize, Serialize};
use std::net::IpAddr;

use super::allowed_address_pair::AllowedAddressPair;
use super::default_true::default_true;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PortSpec {
    pub network: Id,
    /// Chosen on create when absent: the network's first subnet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subnet: Option<Id>,
    /// Allocated on create when empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fixed_ips: Vec<IpAddr>,
    /// Generated on create when empty.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub mac: String,
    /// What the port is plugged into: a machine, or a router for gateway and
    /// interface ports. The id's kind says which.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<Id>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub security_groups: Vec<Id>,
    #[serde(default = "default_true")]
    pub port_security: bool,
    /// Addresses beyond the fixed ones the port may carry. Needs port
    /// security: with it off the port filters nothing anyway.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_address_pairs: Vec<AllowedAddressPair>,
}

impl Default for PortSpec {
    fn default() -> Self {
        Self {
            network: Id::new(kind::NETWORK),
            subnet: None,
            fixed_ips: Vec::new(),
            mac: String::new(),
            device: None,
            security_groups: Vec::new(),
            port_security: true,
            allowed_address_pairs: Vec::new(),
        }
    }
}
