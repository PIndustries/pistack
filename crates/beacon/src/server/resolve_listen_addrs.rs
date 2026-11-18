// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use super::default_port::DEFAULT_PORT;
use super::parse_listen_list::parse_listen_list;
use super::private_ip_from_hint::private_ip_from_hint;

/// Pure resolver used by [`listen_addrs`] and unit tests.
pub fn resolve_listen_addrs(
    env_value: Option<&str>,
    hint: Option<&str>,
) -> anyhow::Result<Vec<SocketAddr>> {
    if let Some(raw) = env_value.map(str::trim).filter(|s| !s.is_empty()) {
        return parse_listen_list(raw);
    }
    if let Some(ip) = private_ip_from_hint(hint.unwrap_or("")) {
        return Ok(vec![SocketAddr::new(IpAddr::V4(ip), DEFAULT_PORT)]);
    }
    Ok(vec![SocketAddr::new(
        IpAddr::V4(Ipv4Addr::LOCALHOST),
        DEFAULT_PORT,
    )])
}
