// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::overlay::checks::canonical_ip::canonical_ip;
use crate::overlay::checks::canonical_mac::canonical_mac;
use crate::overlay::checks::canonical_rules::canonical_rules;
use crate::overlay::checks::unicast::unicast;
use crate::overlay::types::plan_error::PlanError;
use crate::overlay::types::plan_network::PlanNetwork;
use crate::overlay::types::port_address::PortAddress;
use crate::overlay::types::port_endpoint::PortEndpoint;
use std::collections::BTreeSet;
use std::net::Ipv4Addr;

pub(super) fn plan_ports(
    net: &PlanNetwork,
    local: Ipv4Addr,
    peers: &mut BTreeSet<Ipv4Addr>,
    seen_port: &mut BTreeSet<String>,
) -> Result<Vec<PortEndpoint>, PlanError> {
    let mut ports = Vec::with_capacity(net.ports.len());
    let mut seen_mac = BTreeSet::new();
    let mut seen_addr = BTreeSet::new();
    for port in &net.ports {
        if port.id.is_empty() {
            return Err(PlanError::Invalid("port id is empty".into()));
        }
        if !seen_port.insert(port.id.clone()) {
            return Err(PlanError::Invalid(format!("duplicate port {}", port.id)));
        }
        if port.underlay.is_empty() {
            return Err(PlanError::Invalid(format!(
                "port {} has an empty underlay",
                port.id
            )));
        }
        let owned = unicast("port underlay", &port.underlay)?;
        if owned != local {
            peers.insert(owned);
        }
        let mac = canonical_mac(&port.mac)?;
        if !seen_mac.insert(mac.clone()) {
            return Err(PlanError::Invalid(format!(
                "duplicate mac {mac} on {}",
                net.id
            )));
        }
        let address_ip = canonical_ip("port address", &port.address)?;
        if !seen_addr.insert(address_ip) {
            return Err(PlanError::Invalid(format!(
                "duplicate address {address_ip} on {}",
                net.id
            )));
        }
        let mut extra = Vec::with_capacity(port.extra.len());
        for more in &port.extra {
            let ip = canonical_ip("port extra address", &more.address)?;
            if !seen_addr.insert(ip) {
                return Err(PlanError::Invalid(format!(
                    "duplicate address {ip} on {}",
                    net.id
                )));
            }
            let ingress = canonical_rules(&more.ingress)?;
            let egress = canonical_rules(&more.egress)?;
            if ingress
                .iter()
                .chain(&egress)
                .any(|rule| rule.cidr.contains(':') != ip.is_ipv6())
            {
                return Err(PlanError::Invalid(format!(
                    "port {} address {ip} has a rule in the other family",
                    port.id
                )));
            }
            extra.push(PortAddress {
                address: ip.to_string(),
                ingress,
                egress,
            });
        }
        ports.push(PortEndpoint {
            id: port.id.clone(),
            mac,
            address: address_ip.to_string(),
            underlay: owned.to_string(),
            ingress: canonical_rules(&port.ingress)?,
            egress: canonical_rules(&port.egress)?,
            extra,
        });
    }
    Ok(ports)
}
