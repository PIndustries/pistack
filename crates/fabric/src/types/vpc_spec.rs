// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use ipnet::IpNet;
use pistack_types::Id;
use serde::{Deserialize, Serialize};

use super::static_route::StaticRoute;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct VpcSpec {
    /// The blocks this space is made of. Subnets are cut from them. IPv4 or
    /// IPv6, and none of them overlaps another.
    pub cidrs: Vec<IpNet>,
    /// Where traffic leaves and what floating IPs are drawn from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_network: Option<Id>,
    /// Handed to a subnet on one of this VPC's networks that sets none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dns_domain: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub routes: Vec<StaticRoute>,
}
