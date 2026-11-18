// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::SubnetSpec;
use ipnet::IpNet;
use std::net::IpAddr;

/// Addresses never handed out: the network address, the broadcast address
/// (IPv4 only) and the gateway.
pub fn is_reserved(spec: &SubnetSpec, ip: IpAddr) -> bool {
    if spec.gateway == Some(ip) {
        return true;
    }
    match spec.cidr {
        IpNet::V4(n) => n.network() == ip || n.broadcast() == ip,
        IpNet::V6(n) => n.network() == ip,
    }
}
