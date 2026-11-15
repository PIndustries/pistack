// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use fabric::{Port, Subnet};

pub(in crate::overlay) fn address_on_subnet(port: &Port, subnet: &Subnet) -> Option<String> {
    let want_v4 = subnet.spec.cidr.addr().is_ipv4();
    port.spec
        .fixed_ips
        .iter()
        .find(|ip| ip.is_ipv4() == want_v4 && subnet.spec.cidr.contains(*ip))
        .map(|ip| ip.to_string())
}
