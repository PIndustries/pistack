// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The dialable backend and the members that may become one.
//!
//! A [`Backend`] is built from a member, never by hand outside tests, so
//! the weight and the health rules travel together. [`eligible`] and
//! [`dialable`] are the same question asked at two heights: which members
//! a pool may hand to a data plane.

use crate::types::{Member, OperatingStatus};
use std::net::SocketAddr;

/// One dialable backend with what the picker needs to weigh it.
///
/// Built from a member with [`Backend::of`] so the weight and the health
/// rules cannot be forgotten.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Backend {
    pub addr: SocketAddr,
    pub weight: u16,
    /// Connections open on this backend right now. Only `least_conn` uses
    /// it; 0 is right when the caller is not counting.
    pub conns: u64,
}

impl Backend {
    /// A backend the picker may weigh by hand.
    pub fn new(addr: SocketAddr) -> Self {
        Self {
            addr,
            weight: 1,
            conns: 0,
        }
    }

    /// The backend for `member`: its address and weight, and no
    /// connections until something opens one.
    pub fn of(member: &Member) -> Self {
        Self {
            addr: member.addr(),
            weight: member.spec.weight,
            conns: 0,
        }
    }
}

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
