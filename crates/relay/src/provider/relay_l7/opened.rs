// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Counting a connection open on a member.

use std::collections::HashMap;
use std::net::SocketAddr;

pub(super) fn opened(conns: &parking_lot::Mutex<HashMap<SocketAddr, u64>>, addr: SocketAddr) {
    *conns.lock().entry(addr).or_insert(0) += 1;
}
