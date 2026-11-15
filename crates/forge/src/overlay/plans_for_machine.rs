// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::overlay::{all_plans, store_err};
use crate::types::Machine;
use fabric::overlay::OverlayPlan;
use fabric::{FloatingIp, Network, Port, Router, Subnet};
use pistack_store::Store;
use pistack_types::{ApiError, ApiResult, Id};
use serde_json::Value;
use std::collections::BTreeSet;

/// Plans for the networks named on `machine`, plus an external segment that
/// carries a floating binding or the router for one of those networks.
pub fn plans_for_machine(store: &Store, machine: &Machine) -> ApiResult<Vec<(String, Value)>> {
    let names: BTreeSet<String> = machine
        .spec
        .networks
        .iter()
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty())
        .collect();
    if names.is_empty() {
        return Ok(Vec::new());
    }
    let networks = store
        .collection::<Network>()
        .list(None)
        .map_err(store_err)?;
    let tenant: BTreeSet<String> = networks
        .iter()
        .filter(|n| names.contains(&n.meta.name) || names.contains(&n.meta.id.to_string()))
        .map(|n| n.meta.id.to_string())
        .collect();
    if tenant.is_empty() {
        return Ok(Vec::new());
    }
    let subnets = store.collection::<Subnet>().list(None).map_err(store_err)?;
    let routers = store.collection::<Router>().list(None).map_err(store_err)?;
    let ports = store.collection::<Port>().list(None).map_err(store_err)?;
    let fips = store
        .collection::<FloatingIp>()
        .list(None)
        .map_err(store_err)?;
    let mut keep = tenant.clone();
    for router in &routers {
        let attached = router.spec.interfaces.iter().any(|sid| {
            subnets
                .iter()
                .any(|s| &s.meta.id == sid && tenant.contains(&s.spec.network.to_string()))
        });
        if attached {
            if let Some(ext) = &router.spec.external_network {
                keep.insert(ext.to_string());
            }
        }
    }
    let machine_ports: BTreeSet<&Id> = ports
        .iter()
        .filter(|p| p.spec.device.as_ref() == Some(&machine.meta.id))
        .map(|p| &p.meta.id)
        .collect();
    for fip in &fips {
        if fip
            .spec
            .port
            .as_ref()
            .is_some_and(|p| machine_ports.contains(p))
        {
            keep.insert(fip.spec.network.to_string());
        }
    }
    let mut out = Vec::new();
    for (node, value) in all_plans(store)? {
        let mut plan: OverlayPlan = serde_json::from_value(value)
            .map_err(|e| ApiError::internal(format!("overlay plan: {e}")))?;
        plan.networks.retain(|n| keep.contains(&n.id));
        if plan.networks.is_empty() {
            continue;
        }
        let value = serde_json::to_value(&plan)
            .map_err(|e| ApiError::internal(format!("overlay plan: {e}")))?;
        out.push((node, value));
    }
    Ok(out)
}
