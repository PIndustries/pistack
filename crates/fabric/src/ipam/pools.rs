// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::{AllocationPool, SubnetSpec};
use ipnet::IpNet;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// The subnet's effective pools: what the user gave, or the whole range.
pub fn pools(spec: &SubnetSpec) -> Vec<AllocationPool> {
    if !spec.allocation_pools.is_empty() {
        return spec.allocation_pools.clone();
    }
    let (start, end) = match spec.cidr {
        IpNet::V4(n) => {
            if n.prefix_len() >= 31 {
                (IpAddr::V4(n.network()), IpAddr::V4(n.broadcast()))
            } else {
                let s = u32::from(n.network()).saturating_add(1);
                let e = u32::from(n.broadcast()).saturating_sub(1);
                (IpAddr::V4(Ipv4Addr::from(s)), IpAddr::V4(Ipv4Addr::from(e)))
            }
        }
        IpNet::V6(n) => {
            let s = u128::from(n.network()).saturating_add(1);
            (IpAddr::V6(Ipv6Addr::from(s)), IpAddr::V6(n.broadcast()))
        }
    };
    vec![AllocationPool { start, end }]
}
