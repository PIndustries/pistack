// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::overlay::ServiceRecord;
use fabric::overlay::{ServiceBackend, ServiceRule};
use fabric::{Router, Subnet};
use std::net::{IpAddr, Ipv4Addr};

/// Cluster addresses the router answers for services in its project. Each
/// leg lists only the ready workloads on its own subnet; the helper joins
/// the legs of one namespace. A service with none here is still listed, so
/// the router refuses it instead of letting it hang. IPv4 and TCP or UDP
/// only.
pub(in crate::overlay) fn service_rules(
    router: &Router,
    subnet: &Subnet,
    services: &[ServiceRecord],
) -> Vec<ServiceRule> {
    let mut out: Vec<ServiceRule> = Vec::new();
    for svc in services {
        if svc.meta.project != router.meta.project {
            continue;
        }
        let Some(address) = svc
            .status
            .cluster_ip
            .as_deref()
            .and_then(|ip| ip.parse::<Ipv4Addr>().ok())
        else {
            continue;
        };
        let local: Vec<Ipv4Addr> = svc
            .status
            .endpoints
            .iter()
            .filter_map(|e| e.ip.parse::<Ipv4Addr>().ok())
            .filter(|ip| subnet.spec.cidr.contains(&IpAddr::V4(*ip)))
            .collect();
        for port in &svc.spec.ports {
            let protocol = match port.protocol.to_ascii_lowercase().as_str() {
                "" | "tcp" => "tcp",
                "udp" => "udp",
                _ => continue,
            };
            if port.port == 0 {
                continue;
            }
            let target = port.target_port.filter(|p| *p != 0).unwrap_or(port.port);
            let rule = ServiceRule {
                id: svc.meta.id.to_string(),
                address: address.to_string(),
                protocol: protocol.into(),
                port: port.port,
                backends: local
                    .iter()
                    .map(|ip| ServiceBackend {
                        address: ip.to_string(),
                        port: target,
                    })
                    .collect(),
            };
            if out.iter().any(|r| {
                r.address == rule.address && r.protocol == rule.protocol && r.port == rule.port
            }) {
                continue;
            }
            out.push(rule);
        }
    }
    out
}
