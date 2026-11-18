// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::{Host, MachineSpec};
use pistack_types::{ApiResult, Id, Resources};

use super::allocations::Allocations;
use super::place::place;
use super::place_on_ovn::place_on_ovn;
use super::place_on_segment::place_on_segment;
use super::strategy::Strategy;

/// Place a machine among `hosts` by the rule its networks call for: OVN
/// chassis when it names a network and the networks are OVN, hosts on the
/// underlay when it names one otherwise, any host when it names none.
pub(crate) fn place_networked(
    spec: &MachineSpec,
    size: &Resources,
    hosts: &[Host],
    allocations: &Allocations,
    strategy: Strategy,
) -> ApiResult<Id> {
    let named_network = spec.networks.iter().any(|n| !n.trim().is_empty());
    if named_network && crate::ovn::enabled() {
        place_on_ovn(spec, size, hosts, allocations, strategy)
    } else if named_network {
        place_on_segment(spec, size, hosts, allocations, strategy)
    } else {
        place(spec, size, hosts, allocations, strategy)
    }
}
