// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::net::Ipv6Addr;

/// Unique-local IPv6 (`fc00::/7`). Not link-local, not global, and not an underlay.
pub fn is_private_ula(ip: Ipv6Addr) -> bool {
    let [first, ..] = ip.octets();
    first & 0xfe == 0xfc
}
