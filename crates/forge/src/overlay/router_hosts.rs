// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::overlay::{Guest, SeenHost};
use fabric::{Network, Router, Subnet};

/// The host that receives the project router: the lowest underlay among
/// guests on each tenant network the router interfaces.
pub(in crate::overlay) fn router_hosts(
    external: &Network,
    routers: &[Router],
    subnets: &[Subnet],
    guests: &[Guest],
) -> Vec<SeenHost> {
    let mut out = Vec::new();
    for router in routers {
        if router.spec.external_network.as_ref() != Some(&external.meta.id) {
            continue;
        }
        for sid in &router.spec.interfaces {
            let Some(sub) = subnets.iter().find(|s| &s.meta.id == sid) else {
                continue;
            };
            let here: Vec<&Guest> = guests
                .iter()
                .filter(|g| g.port.spec.network == sub.spec.network)
                .collect();
            let Some(underlay) = here.iter().map(|g| g.underlay).min() else {
                continue;
            };
            let Some(sample) = here.iter().find(|g| g.underlay == underlay) else {
                continue;
            };
            out.push(SeenHost {
                underlay,
                node: sample.node.clone(),
            });
        }
    }
    out
}
