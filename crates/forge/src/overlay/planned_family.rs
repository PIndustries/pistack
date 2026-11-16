// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use fabric::{Port, Subnet};

/// Address family of the subnet this port was allocated from, when known.
pub(in crate::overlay) fn planned_family(port: &Port, subnets: &[Subnet]) -> Option<bool> {
    port.spec
        .subnet
        .as_ref()
        .and_then(|id| subnets.iter().find(|s| &s.meta.id == id))
        .map(|s| s.spec.cidr.addr().is_ipv4())
}
