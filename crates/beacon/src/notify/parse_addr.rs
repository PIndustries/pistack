// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::net::{IpAddr, SocketAddr};

/// A `ip` or `ip:port` string as written in a zone spec, with `default_port`
/// filled in for the bare form.
pub fn parse_addr(raw: &str, default_port: u16) -> anyhow::Result<SocketAddr> {
    let s = raw.trim();
    if let Ok(addr) = s.parse::<SocketAddr>() {
        return Ok(addr);
    }
    match s.parse::<IpAddr>() {
        Ok(ip) => Ok(SocketAddr::new(ip, default_port)),
        Err(_) => anyhow::bail!("{raw} is not an address"),
    }
}
