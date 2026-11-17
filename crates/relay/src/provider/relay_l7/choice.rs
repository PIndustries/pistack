// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! What a route lookup came back with.

use std::net::SocketAddr;

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Choice {
    Member(SocketAddr),
    /// A route or the default pool applies but has no member up.
    Unavailable,
    /// Routes exist, none matches, and there is no default member.
    NotFound,
}
