// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use ipnet::IpNet;
use std::net::IpAddr;

pub(crate) fn broadcast(cidr: IpNet) -> Option<IpAddr> {
    match cidr {
        IpNet::V4(n) => Some(IpAddr::V4(n.broadcast())),
        IpNet::V6(_) => None,
    }
}
