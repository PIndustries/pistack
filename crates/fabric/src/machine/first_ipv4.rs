// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::net::IpAddr;

pub(crate) fn first_ipv4(addresses: &[String]) -> Option<IpAddr> {
    addresses.iter().find_map(|raw| {
        let bare = raw.split('/').next()?.trim();
        let ip: IpAddr = bare.parse().ok()?;
        ip.is_ipv4().then_some(ip)
    })
}
