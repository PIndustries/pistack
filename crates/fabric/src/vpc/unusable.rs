// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use ipnet::IpNet;
use std::net::IpAddr;

/// Addresses no host may hold: the network address, and the broadcast address
/// of an IPv4 block with room for one.
pub(crate) fn unusable(cidr: IpNet, addr: IpAddr) -> bool {
    match cidr {
        IpNet::V4(n) => n.network() == addr || (n.prefix_len() < 31 && n.broadcast() == addr),
        IpNet::V6(n) => n.network() == addr,
    }
}
