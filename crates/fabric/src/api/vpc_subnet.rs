// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use serde::Deserialize;
use std::net::IpAddr;

use super::default_dhcp::default_dhcp;

/// The body of `POST /vpcs/{id}/subnets`. The vpc is the one in the path.
#[derive(Deserialize)]
pub struct VpcSubnet {
    pub name: String,
    /// A full cidr, or a prefix length — `24` or `/24` — for the next free
    /// block of that size.
    pub cidr: String,
    #[serde(default)]
    pub gateway: Option<IpAddr>,
    #[serde(default)]
    pub dns_nameservers: Vec<IpAddr>,
    #[serde(default = "default_dhcp")]
    pub enable_dhcp: bool,
}
