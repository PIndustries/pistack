// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use fabric::overlay::is_private_ula;
use fabric::{Router, Subnet};
use std::net::IpAddr;

/// IPv4 external addresses stay as they are. A unique-local IPv6 address is
/// SNAT for an IPv6-only router. A public IPv6 address is not an underlay
/// and is not an external address.
pub(in crate::overlay) fn external_for(
    router: &Router,
    subnet: &Subnet,
    subnets: &[Subnet],
) -> Option<String> {
    match router.status.external_ip? {
        IpAddr::V4(ip) => Some(ip.to_string()),
        IpAddr::V6(ip) => {
            if !is_private_ula(ip) || !subnet.spec.cidr.addr().is_ipv6() {
                return None;
            }
            let mixed = subnets.iter().any(|other| {
                router.spec.interfaces.iter().any(|id| id == &other.meta.id)
                    && other.spec.cidr.addr().is_ipv4()
            });
            if mixed {
                None
            } else {
                Some(ip.to_string())
            }
        }
    }
}
