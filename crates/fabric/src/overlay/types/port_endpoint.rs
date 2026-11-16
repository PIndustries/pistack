// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::overlay::types::port_address::PortAddress;
use crate::overlay::types::sg_rule::SgRule;
use serde::Deserialize;
use serde::Serialize;

/// A guest port, and the underlay host that owns it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortEndpoint {
    pub id: String,
    pub mac: String,
    /// Guest address, no prefix. IPv4 or IPv6.
    pub address: String,
    /// Host that owns this port.
    pub underlay: String,
    pub ingress: Vec<SgRule>,
    pub egress: Vec<SgRule>,
    /// The port's other fixed addresses, each filtered with the rules for
    /// its own family. Empty for a port with one address.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extra: Vec<PortAddress>,
}
