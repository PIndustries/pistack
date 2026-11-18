// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use crate::types::ZoneKind;
use std::net::SocketAddr;

use super::dns_port::DNS_PORT;
use super::parse_addr::parse_addr;
use super::same_apex::same_apex;

/// The addresses a primary zone must notify: every address listed as a
/// primary of a secondary zone for the same apex, plus every address the zone
/// itself names in `also_notify`. The first covers the secondaries held inside
/// this cloud; the second covers the usual outside secondary, an ordinary DNS
/// server that holds nothing in the store and so cannot be discovered. A bare
/// address is taken as the standard DNS port, which is where a peer running on
/// the plain port is. Each address appears once however many places name it.
pub fn peers_for(client: &Client, zone: &crate::types::Zone) -> anyhow::Result<Vec<SocketAddr>> {
    let apex = &zone.spec.name;
    let mut out = Vec::new();
    for raw in &zone.spec.also_notify {
        let addr = parse_addr(raw, DNS_PORT)?;
        if !out.contains(&addr) {
            out.push(addr);
        }
    }
    for z in client.secondary_zones()? {
        if !same_apex(&z.spec.name, apex) {
            continue;
        }
        let ZoneKind::Secondary { primaries } = &z.spec.kind else {
            continue;
        };
        for raw in primaries {
            let addr = parse_addr(raw, DNS_PORT)?;
            if !out.contains(&addr) {
                out.push(addr);
            }
        }
    }
    Ok(out)
}
