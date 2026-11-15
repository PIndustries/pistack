// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::overlay::types::dhcp_lease::DhcpLease;
use crate::overlay::types::dnat_rule::DnatRule;
use crate::overlay::types::service_rule::ServiceRule;

/// A router leg on the network that contains it.
///
/// The same `id` on two networks becomes one [`RouterNamespace`] with two
/// [`RouterInterface`]s.
#[derive(Clone, Debug)]
pub struct PlanRouter {
    pub id: String,
    pub gateway: String,
    pub prefix: u8,
    pub cidr: String,
    pub external_ip: Option<String>,
    pub external_dev: String,
    pub dnat: Vec<DnatRule>,
    pub metadata: bool,
    /// Input only. Copied onto this leg's [`RouterInterface`]. Not serialized.
    pub dhcp: Vec<DhcpLease>,
    /// Service addresses for this router. Legs of one router are merged.
    pub services: Vec<ServiceRule>,
}
