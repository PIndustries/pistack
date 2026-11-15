// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use fabric::{Network, NetworkKind, Subnet};

pub(in crate::overlay) fn shared_nat_network(net: &Network, subnets: &[Subnet]) -> bool {
    matches!(net.spec.kind, NetworkKind::Overlay)
        && subnets
            .iter()
            .any(|s| s.spec.network == net.meta.id && s.spec.cidr.to_string() == "10.88.0.0/24")
}
