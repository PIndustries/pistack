// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::overlay::{float_binding, router_hosts, Guest, Realised, SeenHost};
use fabric::{FloatingIp, Network, Router, Subnet};
use std::collections::BTreeMap;
use std::net::Ipv4Addr;

/// Hosts that must see `net`: a guest on it, a floating binding whose port
/// is on the host, or the designated router that uses it as its external
/// network. The last two apply to provider and external overlay segments.
pub(in crate::overlay) fn segment_hosts(
    net: &Network,
    kind: &Realised,
    guests: &[Guest],
    fips: &[FloatingIp],
    routers: &[Router],
    subnets: &[Subnet],
) -> Vec<SeenHost> {
    let mut nodes: BTreeMap<String, Ipv4Addr> = BTreeMap::new();
    for guest in guests.iter().filter(|g| g.port.spec.network == net.meta.id) {
        nodes.insert(guest.node.clone(), guest.underlay);
    }
    if matches!(kind, Realised::Provider(_) | Realised::External(_)) {
        for fip in fips.iter().filter(|f| f.spec.network == net.meta.id) {
            let Some((underlay, node, _)) = float_binding(fip, guests, routers, subnets) else {
                continue;
            };
            nodes.insert(node, underlay);
        }
        for holder in router_hosts(net, routers, subnets, guests) {
            nodes.insert(holder.node, holder.underlay);
        }
    }
    nodes
        .into_iter()
        .map(|(node, underlay)| SeenHost { underlay, node })
        .collect()
}
