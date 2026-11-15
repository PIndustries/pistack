// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::overlay::Guest;
use crate::types::{usable_underlay, Host, Machine};
use fabric::Port;
use pistack_types::{kind, ApiError, ApiResult};
use std::collections::BTreeMap;
use std::net::Ipv4Addr;

pub(in crate::overlay) fn placed_guests(
    ports: &[Port],
    machines: &[Machine],
    hosts: &[Host],
) -> ApiResult<Vec<Guest>> {
    let mut out = Vec::new();
    let mut underlay_owner: BTreeMap<Ipv4Addr, String> = BTreeMap::new();
    for port in ports {
        if port.is_router_port() {
            continue;
        }
        let Some(device) = port.spec.device.as_ref() else {
            continue;
        };
        if device.kind != kind::MACHINE {
            continue;
        }
        let Some(machine) = machines.iter().find(|m| &m.meta.id == device) else {
            continue;
        };
        let Some(host_id) = machine.status.host.as_ref() else {
            continue;
        };
        let Some(host) = hosts.iter().find(|h| &h.meta.id == host_id) else {
            continue;
        };
        let Some(raw) = usable_underlay(host.spec.underlay_ip.as_deref()) else {
            continue;
        };
        let underlay: Ipv4Addr = raw.parse().map_err(|_| {
            ApiError::internal(format!("underlay {raw} on {} is not ipv4", host.meta.id))
        })?;
        if let Some(prev) = underlay_owner.insert(underlay, host.spec.node_name.clone()) {
            if prev != host.spec.node_name {
                return Err(ApiError::conflict(format!(
                    "underlay {underlay} is claimed by {prev} and {}",
                    host.spec.node_name
                )));
            }
        }
        out.push(Guest {
            port: port.clone(),
            underlay,
            node: host.spec.node_name.clone(),
        });
    }
    Ok(out)
}
