// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::overlay::{real_vni, shared_nat_network};
use fabric::{Network, NetworkKind, Subnet};

/// Floating translation follows the guest port when the external network is
/// itself an overlay segment or a provider network. Anything else, including
/// 10.88.0.0/24, stays as router DNAT.
pub(in crate::overlay) fn follows_port(net: &Network, subnets: &[Subnet]) -> bool {
    if shared_nat_network(net, subnets) {
        return false;
    }
    match &net.spec.kind {
        NetworkKind::Provider { .. } => true,
        NetworkKind::Overlay => real_vni(net).is_some(),
    }
}
