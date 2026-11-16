// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::net::Ipv4Addr;

/// Unicast IPv4. Private, shared (100.64/10), public, and loopback are all
/// accepted. The agent, not the planner, refuses a public NIC.
pub(in crate::overlay) fn usable(ip: Ipv4Addr) -> bool {
    let a = ip.octets()[0];
    !ip.is_unspecified() && !ip.is_broadcast() && !ip.is_multicast() && a != 0 && a < 240
}
