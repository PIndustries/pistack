// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use pistack_types::{kind, ApiResult, Id, Meta};
use std::collections::{BTreeSet, HashMap};
use std::net::IpAddr;

use super::floating_ip_view::FloatingIpView;
use super::machine_view::MachineView;
use super::port_view::PortView;
use super::scan::scan;

/// Machine id -> addresses beacon should publish for it.
pub(crate) fn machine_addresses(client: &Client) -> ApiResult<Vec<(Meta, Vec<IpAddr>)>> {
    let machines: Vec<MachineView> = scan(client, kind::MACHINE)?;
    let ports: Vec<PortView> = scan(client, kind::PORT)?;
    let fips: Vec<FloatingIpView> = scan(client, kind::FLOATING_IP)?;

    let port_device: HashMap<Id, Id> = ports
        .into_iter()
        .filter_map(|p| p.spec.device.map(|d| (p.meta.id, d)))
        .collect();
    let mut floating: HashMap<Id, BTreeSet<IpAddr>> = HashMap::new();
    for f in fips {
        if let (Some(port), Some(addr)) = (f.spec.port, f.spec.address) {
            if let Some(dev) = port_device.get(&port) {
                floating.entry(dev.clone()).or_default().insert(addr);
            }
        }
    }

    let mut out = Vec::with_capacity(machines.len());
    for m in machines {
        let addrs: Vec<IpAddr> = match floating.get(&m.meta.id) {
            Some(f) if !f.is_empty() => f.iter().copied().collect(),
            _ => {
                let mut v: Vec<IpAddr> = m
                    .status
                    .addresses
                    .iter()
                    .filter_map(|a| a.split('/').next()?.trim().parse().ok())
                    .collect();
                v.sort();
                v.dedup();
                v
            }
        };
        out.push((m.meta, addrs));
    }
    Ok(out)
}
