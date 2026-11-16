// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use fabric::Port;
use pistack_types::Id;
use std::net::IpAddr;

/// Host prefixes of every port in `group`, in one address family.
pub(in crate::overlay) fn member_cidrs(group: &Id, v6: bool, ports: &[Port]) -> Vec<String> {
    let mut out = Vec::new();
    for port in ports {
        if !port.spec.security_groups.iter().any(|g| g == group) {
            continue;
        }
        for ip in &port.spec.fixed_ips {
            match ip {
                IpAddr::V4(_) if !v6 => out.push(format!("{ip}/32")),
                IpAddr::V6(_) if v6 => out.push(format!("{ip}/128")),
                _ => {}
            }
        }
    }
    out
}
