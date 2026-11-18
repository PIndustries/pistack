// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use ipnet::IpNet;
use serde::{Deserialize, Serialize};
use std::net::IpAddr;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostRoute {
    pub destination: IpNet,
    pub nexthop: IpAddr,
}
