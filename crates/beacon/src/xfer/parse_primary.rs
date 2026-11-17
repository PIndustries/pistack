// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::net::SocketAddr;
use std::str::FromStr;

/// Parse a primary address from zone spec form (`ip`, `ip:port`, `[ipv6]:port`).
pub(crate) fn parse_primary(raw: &str) -> anyhow::Result<SocketAddr> {
    let s = raw.trim();
    if s.is_empty() {
        anyhow::bail!("empty primary address");
    }
    if let Ok(addr) = SocketAddr::from_str(s) {
        return Ok(addr);
    }
    // Bare IP → port 53.
    if let Ok(ip) = std::net::IpAddr::from_str(s) {
        return Ok(SocketAddr::new(ip, 53));
    }
    anyhow::bail!("primary {raw} is not an address")
}
