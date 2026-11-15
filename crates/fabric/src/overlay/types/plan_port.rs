// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::overlay::types::port_address::PortAddress;
use crate::overlay::types::sg_rule::SgRule;

/// A guest port supplied to [`plan`].
#[derive(Clone, Debug)]
pub struct PlanPort {
    pub id: String,
    pub mac: String,
    pub address: String,
    pub underlay: String,
    pub ingress: Vec<SgRule>,
    pub egress: Vec<SgRule>,
    /// Other fixed addresses on the port, for example the IPv6 address of a
    /// port that also has an IPv4 one.
    pub extra: Vec<PortAddress>,
}
