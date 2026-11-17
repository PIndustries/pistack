// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Which members may take traffic, and the addresses a data plane that
//! dials by address starts from.

use crate::types::{Member, OperatingStatus};
use std::net::SocketAddr;

/// Members that may take traffic: administratively up, weighted above
/// zero, and `Online` or unmonitored. Under every algorithm a member
/// outside that is never a choice, so a data plane that dials addresses
/// rather than members starts from this list too.
pub fn eligible(members: &[Member]) -> Vec<&Member> {
    members
        .iter()
        .filter(|m| {
            m.spec.admin_up
                && m.spec.weight > 0
                && matches!(
                    m.status.operating_status,
                    OperatingStatus::Online | OperatingStatus::NoMonitor
                )
        })
        .collect()
}

/// The addresses of the members that may take traffic, in the same order.
pub fn dialable(members: &[Member]) -> Vec<SocketAddr> {
    eligible(members).into_iter().map(|m| m.addr()).collect()
}
