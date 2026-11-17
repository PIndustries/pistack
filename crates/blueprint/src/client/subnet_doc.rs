// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use serde::Deserialize;
use std::net::IpAddr;

use super::default_true::default_true;

#[derive(Deserialize)]
pub(crate) struct SubnetDoc {
    pub(crate) network: String,
    pub(crate) cidr: String,
    #[serde(default)]
    pub(crate) gateway: Option<IpAddr>,
    #[serde(default)]
    pub(crate) allocation_pools: Vec<fabric::AllocationPool>,
    #[serde(default)]
    pub(crate) dns_nameservers: Vec<IpAddr>,
    #[serde(default = "default_true")]
    pub(crate) enable_dhcp: bool,
    #[serde(default)]
    pub(crate) host_routes: Vec<fabric::HostRoute>,
}
