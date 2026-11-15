// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::overlay::checks::canonical_cidr::canonical_cidr;
use crate::overlay::checks::canonical_ip::canonical_ip;
use crate::overlay::checks::canonical_mac::canonical_mac;
use crate::overlay::types::dhcp_lease::DhcpLease;
use crate::overlay::types::dhcp_route::DhcpRoute;
use crate::overlay::types::plan_error::PlanError;
use ipnet::IpNet;
use std::collections::BTreeSet;

pub(super) fn canonical_dhcp(
    leases: &[DhcpLease],
    cidr: &str,
) -> Result<Vec<DhcpLease>, PlanError> {
    let net: IpNet = cidr
        .parse()
        .map_err(|_| PlanError::Invalid(format!("cidr {cidr} is not an ip network")))?;
    let mut seen_mac = BTreeSet::new();
    let mut seen_addr = BTreeSet::new();
    let mut out = Vec::with_capacity(leases.len());
    for lease in leases {
        let mac = canonical_mac(&lease.mac)?;
        if !seen_mac.insert(mac.clone()) {
            return Err(PlanError::Invalid(format!("duplicate dhcp mac {mac}")));
        }
        let address = canonical_ip("dhcp address", &lease.address)?;
        if !net.contains(&address) {
            return Err(PlanError::Invalid(format!(
                "dhcp address {address} is outside {cidr}"
            )));
        }
        if !seen_addr.insert(address) {
            return Err(PlanError::Invalid(format!(
                "duplicate dhcp address {address}"
            )));
        }
        let mut dns = Vec::with_capacity(lease.dns.len());
        for entry in &lease.dns {
            dns.push(canonical_ip("dns", entry)?.to_string());
        }
        let mut routes = Vec::with_capacity(lease.routes.len());
        for route in &lease.routes {
            routes.push(DhcpRoute {
                destination: canonical_cidr("route destination", &route.destination)?,
                nexthop: canonical_ip("route nexthop", &route.nexthop)?.to_string(),
            });
        }
        out.push(DhcpLease {
            mac,
            address: address.to_string(),
            dns,
            routes,
        });
    }
    Ok(out)
}
