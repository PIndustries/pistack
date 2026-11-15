// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::overlay::{collides_with_router_ip, port_address, Guest};
use fabric::overlay::PlanFloat;
use fabric::{FloatingIp, Router, Subnet};
use std::net::Ipv4Addr;

/// `Some` when this floating address is installed on the external segment of
/// the host that owns the guest port. An address equal to the router
/// external ip is never emitted.
pub(in crate::overlay) fn float_binding(
    fip: &FloatingIp,
    guests: &[Guest],
    routers: &[Router],
    subnets: &[Subnet],
) -> Option<(Ipv4Addr, String, PlanFloat)> {
    let port_id = fip.spec.port.as_ref()?;
    let guest = guests.iter().find(|g| &g.port.meta.id == port_id)?;
    let floating = fip.spec.address?.to_string();
    if collides_with_router_ip(fip, &floating, &guest.port, routers, subnets) {
        return None;
    }
    let fixed = fip
        .status
        .fixed_ip
        .map(|ip| ip.to_string())
        .or_else(|| port_address(&guest.port, subnets))?;
    Some((
        guest.underlay,
        guest.node.clone(),
        PlanFloat {
            id: fip.meta.id.to_string(),
            floating,
            fixed,
        },
    ))
}
