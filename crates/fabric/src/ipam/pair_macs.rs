// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use ipnet::IpNet;

use super::format_mac::format_mac;
use super::parse_mac::parse_mac;

/// The pairs of a port with their MACs canonicalised. A pair that names no
/// MAC, or one fabric could not parse, keeps `None`: the port's own address.
pub fn pair_macs(port: &crate::types::Port) -> Vec<(Option<String>, IpNet)> {
    port.spec
        .allowed_address_pairs
        .iter()
        .map(|p| {
            (
                p.mac_address
                    .as_deref()
                    .and_then(|m| parse_mac(m).ok())
                    .map(format_mac),
                p.ip_address,
            )
        })
        .collect()
}
