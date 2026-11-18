// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::net::Ipv4Addr;

use super::underlay_ip_ok::underlay_ip_ok;

/// Usable underlay address: IPv4 unicast in 10/8, 172.16/12, 192.168/16,
/// or 100.64/10. A missing or rejected value is `None`.
pub(crate) fn usable_underlay(raw: Option<&str>) -> Option<String> {
    let raw = raw?.trim();
    if raw.is_empty() {
        return None;
    }
    let ip: Ipv4Addr = raw.parse().ok()?;
    underlay_ip_ok(ip).then(|| ip.to_string())
}
