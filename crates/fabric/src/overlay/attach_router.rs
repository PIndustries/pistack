// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::overlay::canonical_dhcp::canonical_dhcp;
use crate::overlay::checks::canonical_dnat::canonical_dnat;
use crate::overlay::checks::canonical_external_ip::canonical_external_ip;
use crate::overlay::checks::canonical_services::canonical_services;
use crate::overlay::checks::claim::claim;
use crate::overlay::checks::gateway_in_cidr::gateway_in_cidr;
use crate::overlay::device_hex::device_hex;
use crate::overlay::merge_router::merge_router;
use crate::overlay::types::pending_router::PendingRouter;
use crate::overlay::types::plan_error::PlanError;
use crate::overlay::types::plan_router::PlanRouter;
use crate::overlay::types::router_interface::RouterInterface;
use crate::overlay::types::router_namespace::RouterNamespace;
use std::collections::BTreeSet;

pub(super) fn attach_router(
    pending: &mut Vec<PendingRouter>,
    names: &mut BTreeSet<String>,
    at: usize,
    network_id: &str,
    net_hex: &str,
    router: &PlanRouter,
) -> Result<(), PlanError> {
    if pending
        .iter()
        .any(|p| p.ns.id == router.id && p.ns.interfaces.iter().any(|i| i.network_id == network_id))
    {
        return Err(PlanError::Invalid(format!(
            "router {} is listed twice on {network_id}",
            router.id
        )));
    }
    let rhex = device_hex(&router.id)?;
    let (gateway, cidr) = gateway_in_cidr(&router.gateway, router.prefix, &router.cidr)?;
    let dhcp = canonical_dhcp(&router.dhcp, &cidr)?;
    let external_ip = canonical_external_ip(&router.external_ip)?;
    let dnat = canonical_dnat(&router.dnat)?;
    let services = canonical_services(&router.services)?;
    debug_assert_eq!(net_hex.len(), 12);
    debug_assert_eq!(rhex.len(), 12);
    let veth_key = format!("{}{}", &net_hex[..6], &rhex[..6]);
    let iface = RouterInterface {
        network_id: network_id.to_string(),
        veth_bridge: claim(names, format!("a{veth_key}"), "a")?,
        veth_ns: claim(names, format!("b{veth_key}"), "b")?,
        gateway,
        prefix: router.prefix,
        cidr,
        dhcp,
    };

    if let Some(existing) = pending.iter_mut().find(|p| p.ns.id == router.id) {
        merge_router(&mut existing.ns, router, external_ip, dnat, services)?;
        existing.ns.interfaces.push(iface);
        return Ok(());
    }

    let snat = external_ip.is_some();
    pending.push(PendingRouter {
        at,
        ns: RouterNamespace {
            id: router.id.clone(),
            netns: claim(names, format!("ns{rhex}"), "ns")?,
            interfaces: vec![iface],
            external_veth_root: claim(names, format!("c{rhex}"), "c")?,
            external_veth_ns: claim(names, format!("d{rhex}"), "d")?,
            external_ip,
            external_dev: router.external_dev.clone(),
            snat,
            dnat,
            metadata: router.metadata,
            services,
        },
    });
    Ok(())
}
