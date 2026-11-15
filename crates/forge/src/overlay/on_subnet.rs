// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use fabric::{Port, Subnet};

pub(in crate::overlay) fn on_subnet(port: &Port, subnet: &Subnet) -> bool {
    port.spec.subnet.as_ref() == Some(&subnet.meta.id)
        || port
            .spec
            .fixed_ips
            .iter()
            .any(|ip| subnet.spec.cidr.contains(ip))
}
