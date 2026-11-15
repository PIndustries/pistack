// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::overlay::{address_on_subnet, on_subnet, Guest};
use fabric::overlay::{DhcpLease, DhcpRoute};
use fabric::Subnet;

/// Leases for the designated router host. Other hosts send no router, so
/// they send no leases.
pub(in crate::overlay) fn dhcp_leases(subnet: &Subnet, bound: &[&Guest]) -> Vec<DhcpLease> {
    if !subnet.spec.enable_dhcp {
        return Vec::new();
    }
    let dns: Vec<String> = subnet
        .spec
        .dns_nameservers
        .iter()
        .map(ToString::to_string)
        .collect();
    let routes: Vec<DhcpRoute> = subnet
        .spec
        .host_routes
        .iter()
        .map(|route| DhcpRoute {
            destination: route.destination.to_string(),
            nexthop: route.nexthop.to_string(),
        })
        .collect();
    let mut leases = Vec::new();
    for guest in bound {
        if !on_subnet(&guest.port, subnet) {
            continue;
        }
        let Some(address) = address_on_subnet(&guest.port, subnet) else {
            continue;
        };
        if guest.port.spec.mac.is_empty() {
            continue;
        }
        leases.push(DhcpLease {
            mac: guest.port.spec.mac.clone(),
            address,
            dns: dns.clone(),
            routes: routes.clone(),
        });
    }
    leases
}
