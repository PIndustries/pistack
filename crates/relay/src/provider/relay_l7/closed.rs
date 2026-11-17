// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Counting a connection closed on a member, and forgetting the member once
//! none are left on it.

use std::collections::HashMap;
use std::net::SocketAddr;

pub(super) fn closed(conns: &parking_lot::Mutex<HashMap<SocketAddr, u64>>, addr: SocketAddr) {
    let mut open = conns.lock();
    if let Some(n) = open.get_mut(&addr) {
        *n = n.saturating_sub(1);
        if *n == 0 {
            open.remove(&addr);
        }
    }
}
