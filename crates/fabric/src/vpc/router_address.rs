// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use ipnet::IpNet;
use std::net::IpAddr;

use super::unusable::unusable;

/// The address a router takes on a subnet: the gateway it was given, otherwise
/// the first address of the block that a host could hold.
pub(crate) fn router_address(cidr: IpNet, gateway: Option<IpAddr>) -> Option<IpAddr> {
    if let Some(g) = gateway {
        return Some(g);
    }
    let mut cur = Some(cidr.network());
    while let Some(a) = cur {
        if !unusable(cidr, a) {
            return Some(a);
        }
        cur = crate::ipam::next_ip(a);
    }
    None
}
