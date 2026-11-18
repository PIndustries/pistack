// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::ovn::Refusal;
use crate::types::{Host, MachineSpec};
use pistack_types::{ApiError, ApiResult, Id, Resources};

use super::allocations::Allocations;
use super::place::place;
use super::strategy::Strategy;

/// Place a machine that named a network when the networks are OVN.
///
/// A host qualifies when it reported a usable underlay address and its last
/// heartbeat said it is a working OVN chassis. When none qualify, the error
/// says how many hosts lacked each. Otherwise this is [`place`] on the hosts
/// that remain, including that function's resource and driver errors.
pub fn place_on_ovn(
    spec: &MachineSpec,
    size: &Resources,
    hosts: &[Host],
    allocations: &Allocations,
    strategy: Strategy,
) -> ApiResult<Id> {
    let mut no_underlay = 0u64;
    let mut not_ready = 0u64;
    let mut usable = Vec::new();
    for h in hosts {
        match crate::ovn::not_usable(h) {
            None => usable.push(h.clone()),
            Some(Refusal::NoUnderlay) => no_underlay += 1,
            Some(Refusal::NotReady) => not_ready += 1,
        }
    }
    if usable.is_empty() {
        return Err(ApiError::unavailable(format!(
            "no host is ready for an OVN network: {} hosts, {no_underlay} lacked an underlay, {not_ready} are not a connected OVN chassis",
            hosts.len()
        ))
        .with_detail("hosts", hosts.len() as u64)
        .with_detail("no_underlay", no_underlay)
        .with_detail("ovn_not_ready", not_ready));
    }
    place(spec, size, &usable, allocations, strategy)
}
