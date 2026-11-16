// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::overlay::types::dhcp_lease::DhcpLease;
use serde::Deserialize;
use serde::Serialize;

/// A router leg on one tenant network.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouterInterface {
    pub network_id: String,
    /// Bridge side, in the root namespace, enslaved to the network bridge.
    pub veth_bridge: String,
    /// Namespace side. Holds the gateway.
    pub veth_ns: String,
    pub gateway: String,
    pub prefix: u8,
    pub cidr: String,
    /// Leases answered on this leg. Omitted when empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dhcp: Vec<DhcpLease>,
}
