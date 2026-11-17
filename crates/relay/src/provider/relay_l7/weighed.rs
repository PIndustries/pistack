// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The backends as the picker should see them, each with the connections
//! this proxy already holds on it.

use crate::picker::Backend;
use std::collections::HashMap;
use std::net::SocketAddr;

/// One backend with the connections this proxy holds on it.
pub(super) fn weighed(
    backends: &[Backend],
    conns: &parking_lot::Mutex<HashMap<SocketAddr, u64>>,
) -> Vec<Backend> {
    let open = conns.lock();
    backends
        .iter()
        .map(|b| Backend {
            conns: open.get(&b.addr).copied().unwrap_or(0),
            ..*b
        })
        .collect()
}
