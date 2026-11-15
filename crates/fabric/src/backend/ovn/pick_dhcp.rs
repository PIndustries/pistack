// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::super::ovsdb;
use super::super::ovsdb::uuid;
use serde_json::Value;
use std::net::IpAddr;

/// The DHCP options row on `net` whose range holds one of `ips`, per family.
pub(in crate::backend::ovn) fn pick_dhcp(rows: &[Value], ips: &[IpAddr], v4: bool) -> Vec<Value> {
    rows.iter()
        .filter_map(|r| {
            let cidr: ipnet::IpNet = r.get("cidr")?.as_str()?.parse().ok()?;
            (cidr.addr().is_ipv4() == v4 && ips.iter().any(|ip| cidr.contains(ip)))
                .then(|| ovsdb::row_uuid(r))
                .flatten()
        })
        .take(1)
        .map(|u| uuid(&u))
        .collect()
}
