// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The member a new client goes to: the members in turn.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};

pub(super) fn pick(members: &[SocketAddr], cursor: &AtomicUsize) -> SocketAddr {
    let n = cursor.fetch_add(1, Ordering::Relaxed);
    members[n % members.len()]
}
