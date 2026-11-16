// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::overlay::{float_binding, follows_port, Guest};
use fabric::overlay::PlanFloat;
use fabric::{FloatingIp, Network, Router, Subnet};
use std::net::Ipv4Addr;

pub(in crate::overlay) fn floats_for(
    net: &Network,
    underlay: Ipv4Addr,
    guests: &[Guest],
    fips: &[FloatingIp],
    routers: &[Router],
    subnets: &[Subnet],
) -> Vec<PlanFloat> {
    if !follows_port(net, subnets) {
        return Vec::new();
    }
    let mut out = Vec::new();
    for fip in fips.iter().filter(|f| f.spec.network == net.meta.id) {
        let Some((host, _, float)) = float_binding(fip, guests, routers, subnets) else {
            continue;
        };
        if host == underlay {
            out.push(float);
        }
    }
    out
}
