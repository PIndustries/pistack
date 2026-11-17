// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Which `http://` addresses a door may be told to publish.

use std::net::Ipv4Addr;

pub(crate) fn allowed_upstream(upstream: &str) -> bool {
    let Some(rest) = upstream.strip_prefix("http://") else {
        return false;
    };
    if rest.is_empty() || rest.contains(['/', '?', '#', '@', '[', ']']) {
        return false;
    }
    let (host, has_port) = match rest.split_once(':') {
        Some((host, port)) => {
            if host.is_empty() || port.is_empty() || port.contains(':') {
                return false;
            }
            match port.parse::<u16>() {
                Ok(0) | Err(_) => return false,
                Ok(_) => (host, true),
            }
        }
        None => (rest, false),
    };
    let Ok(ip) = host.parse::<Ipv4Addr>() else {
        return false;
    };
    if ip == Ipv4Addr::LOCALHOST {
        return has_port;
    }
    let [a, b, _, _] = ip.octets();
    matches!((a, b), (10, _) | (192, 168) | (172, 16..=31))
}
