// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use fabric::Port;

pub(in crate::overlay) fn choose_address(port: &Port, want_v4: Option<bool>) -> Option<String> {
    let ips = &port.spec.fixed_ips;
    if let Some(v4) = want_v4 {
        if let Some(ip) = ips.iter().find(|ip| ip.is_ipv4() == v4) {
            return Some(ip.to_string());
        }
    }
    ips.iter()
        .find(|ip| ip.is_ipv4())
        .or_else(|| ips.first())
        .map(|ip| ip.to_string())
}
