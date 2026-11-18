// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::{usable_underlay, Host, MachineSpec};
use pistack_types::{ApiError, ApiResult, Id, Resources};

use super::allocations::Allocations;
use super::place::place;
use super::strategy::Strategy;

/// Place a machine that named a network onto a host on the underlay.
///
/// Hosts whose `underlay_ip` is missing or not a private unicast IPv4 are
/// not on the segment. When none remain, the error names the segment and
/// how many hosts lacked an underlay. Otherwise this is [`place`] on the
/// hosts that remain, including that function's resource and driver errors.
pub fn place_on_segment(
    spec: &MachineSpec,
    size: &Resources,
    hosts: &[Host],
    allocations: &Allocations,
    strategy: Strategy,
) -> ApiResult<Id> {
    let on_segment: Vec<Host> = hosts
        .iter()
        .filter(|h| usable_underlay(h.spec.underlay_ip.as_deref()).is_some())
        .cloned()
        .collect();
    if on_segment.is_empty() {
        let lacked = hosts.len();
        return Err(ApiError::unavailable(format!(
            "no host is on the network segment ({lacked} hosts lacked an underlay)"
        )));
    }
    place(spec, size, &on_segment, allocations, strategy)
}
