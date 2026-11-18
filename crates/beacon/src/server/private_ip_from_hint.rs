// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::net::{IpAddr, Ipv4Addr};

use super::host_from_hint::host_from_hint;

/// Private unicast IPv4 from a `host:port` or URL host. Matches cluster
/// advertise: RFC 1918 only; loopback, unspecified, public, and names are
/// refused so defaults never bind every interface or a public address.
pub(crate) fn private_ip_from_hint(hint: &str) -> Option<Ipv4Addr> {
    let host = host_from_hint(hint)?;
    let ip: IpAddr = host.parse().ok()?;
    let IpAddr::V4(v4) = ip else {
        return None;
    };
    if v4.is_private() && !v4.is_unspecified() && !v4.is_broadcast() && !v4.is_multicast() {
        Some(v4)
    } else {
        None
    }
}
