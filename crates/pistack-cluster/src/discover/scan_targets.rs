// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::net::Ipv4Addr;

use super::discover_error::DiscoverError;

/// Hosts in `ip/prefix`, skipping the network, the broadcast, and `ip`.
/// Prefixes shorter than /24 are refused. /32 returns an empty list.
pub fn scan_targets(ip: Ipv4Addr, prefix: u8) -> Result<Vec<Ipv4Addr>, DiscoverError> {
    if !(24..=32).contains(&prefix) {
        return Err(DiscoverError::Prefix(prefix));
    }
    if prefix == 32 {
        return Ok(Vec::new());
    }
    let width = 32 - prefix;
    let size = 1u32 << width;
    let base = u32::from(ip) & !(size - 1);
    let mut out = Vec::with_capacity(size as usize);
    for offset in 1..size - 1 {
        let host = Ipv4Addr::from(base + offset);
        if host != ip {
            out.push(host);
        }
    }
    Ok(out)
}
