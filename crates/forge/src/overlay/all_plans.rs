// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::overlay::{
    attach_float_ports, classify, floats_for, placed_guests, port_endpoint, realise,
    refused_physnet, routers_on, segment_hosts, store_err, Guest, Realised, ServiceRecord,
};
use crate::types::{Host, Machine};
use fabric::overlay::{OverlayPlan, PlanNetwork, SegmentPlan, PLAN_VERSION};
use fabric::{FloatingIp, Network, Port, Router, SecurityGroup, Subnet};
use pistack_store::Store;
use pistack_types::{ApiError, ApiResult};
use serde_json::Value;
use std::collections::BTreeMap;

/// Plans for every host that holds a guest, a project router, or a floating
/// binding on an overlay segment or a provider network.
pub fn all_plans(store: &Store) -> ApiResult<Vec<(String, Value)>> {
    let hosts = store.collection::<Host>().list(None).map_err(store_err)?;
    let machines = store
        .collection::<Machine>()
        .list(None)
        .map_err(store_err)?;
    let networks = store
        .collection::<Network>()
        .list(None)
        .map_err(store_err)?;
    let subnets = store.collection::<Subnet>().list(None).map_err(store_err)?;
    let ports = store.collection::<Port>().list(None).map_err(store_err)?;
    let routers = store.collection::<Router>().list(None).map_err(store_err)?;
    let groups = store
        .collection::<SecurityGroup>()
        .list(None)
        .map_err(store_err)?;
    let fips = store
        .collection::<FloatingIp>()
        .list(None)
        .map_err(store_err)?;
    let services = store
        .collection::<ServiceRecord>()
        .list(None)
        .map_err(store_err)?;
    let guests = placed_guests(&ports, &machines, &hosts)?;

    let mut by_node: BTreeMap<String, (String, Vec<SegmentPlan>)> = BTreeMap::new();
    for net in &networks {
        let Some(kind) = classify(net, &subnets) else {
            continue;
        };
        let seeing = segment_hosts(net, &kind, &guests, &fips, &routers, &subnets);
        if seeing.is_empty() {
            continue;
        }
        if let Realised::Provider(provider) = &kind {
            if refused_physnet(&provider.physnet) {
                return Err(ApiError::unprocessable(format!(
                    "refusing physnet {}",
                    provider.physnet
                )));
            }
        }
        let on_net: Vec<&Guest> = guests
            .iter()
            .filter(|g| g.port.spec.network == net.meta.id)
            .collect();
        let designated = on_net.iter().map(|g| g.underlay).min();
        let plan_ports = on_net
            .iter()
            .map(|g| port_endpoint(g, &groups, &ports, &subnets))
            .collect::<ApiResult<Vec<_>>>()?;
        let routers_here = match designated {
            Some(_) => routers_on(
                &net.meta.id,
                &networks,
                &subnets,
                &routers,
                &ports,
                &fips,
                &services,
                &on_net,
            )?,
            None => Vec::new(),
        };
        let (vni, provider) = match &kind {
            Realised::Tenant(vni) | Realised::External(vni) => (*vni, None),
            // A provider segment is not a VXLAN. The VNI stays 0.
            Realised::Provider(provider) => (0, Some(provider.clone())),
        };
        let force_metadata = matches!(kind, Realised::Tenant(_));
        for member in &seeing {
            let floating = floats_for(net, member.underlay, &guests, &fips, &routers, &subnets);
            let routers = if designated == Some(member.underlay) {
                routers_here.clone()
            } else {
                Vec::new()
            };
            // The fixed address has to be a port on the segment that hosts
            // the floating binding. A guest port lives on its tenant network,
            // so the external segment lists it too.
            let mut ports_here = plan_ports.clone();
            attach_float_ports(
                &mut ports_here,
                &floating,
                &guests,
                &groups,
                &ports,
                &subnets,
            )?;
            let segment = realise(
                member.underlay,
                PlanNetwork {
                    id: net.meta.id.to_string(),
                    vni,
                    mtu: net.spec.mtu,
                    ports: ports_here,
                    routers,
                    provider: provider.clone(),
                    floating,
                },
                force_metadata,
            )?;
            by_node
                .entry(member.node.clone())
                .or_insert_with(|| (member.underlay.to_string(), Vec::new()))
                .1
                .push(segment);
        }
    }

    let mut out = Vec::with_capacity(by_node.len());
    for (node, (underlay, networks)) in by_node {
        let doc = OverlayPlan {
            version: PLAN_VERSION,
            host: underlay,
            networks,
        };
        let value = serde_json::to_value(&doc)
            .map_err(|e| ApiError::internal(format!("overlay plan: {e}")))?;
        out.push((node, value));
    }
    Ok(out)
}
