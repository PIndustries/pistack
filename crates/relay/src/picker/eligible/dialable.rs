// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The addresses a data plane that dials by address starts from.

use super::members::eligible;
use crate::types::Member;
use std::net::SocketAddr;

/// The addresses of the members that may take traffic, in the same order.
pub fn dialable(members: &[Member]) -> Vec<SocketAddr> {
    eligible(members).into_iter().map(|m| m.addr()).collect()
}
