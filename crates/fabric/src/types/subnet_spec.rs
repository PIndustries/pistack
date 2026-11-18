// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use ipnet::IpNet;
use pistack_types::Id;
use serde::{Deserialize, Serialize};
use std::net::IpAddr;

use super::allocation_pool::AllocationPool;
use super::default_true::default_true;
use super::host_route::HostRoute;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SubnetSpec {
    pub network: Id,
    pub cidr: IpNet,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gateway: Option<IpAddr>,
    /// Empty means "the whole CIDR minus network, broadcast and gateway".
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allocation_pools: Vec<AllocationPool>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dns_nameservers: Vec<IpAddr>,
    #[serde(default = "default_true")]
    pub enable_dhcp: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub host_routes: Vec<HostRoute>,
}
