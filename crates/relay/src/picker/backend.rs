// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The dialable backend: the address to dial with the weight and open
//! connections the picker needs.
//!
//! A [`Backend`] is built from a member, never by hand outside tests, so
//! the weight and the health rules travel together. Which members may
//! become one is the question in [`super::eligible`].

use crate::types::Member;
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
