// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! What the forwarder dials: the members that may take traffic, with their
//! weights, plus the connections open on each so `least_conn` can see them.

use crate::picker::{Backend, Picker};
use crate::types::{Algorithm, Member};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

#[derive(Clone)]
pub(super) struct Dial {
    pub(super) backends: Arc<parking_lot::Mutex<Vec<Backend>>>,
    pub(super) picker: Picker,
    pub(super) conns: Arc<parking_lot::Mutex<HashMap<SocketAddr, u64>>>,
}

impl Dial {
    pub(super) fn new(algo: Algorithm) -> Self {
        Self {
            backends: Arc::new(parking_lot::Mutex::new(Vec::new())),
            picker: Picker::new(algo),
            conns: Arc::new(parking_lot::Mutex::new(HashMap::new())),
        }
    }

    /// Replace the member list. A member re-added at the same address
    /// starts counting again.
    pub(super) fn set_members(&self, members: &[Member]) {
        *self.backends.lock() = members.iter().map(Backend::of).collect();
    }

    /// The backends to choose between, each with its live connection count.
    pub(super) fn snapshot(&self) -> Vec<Backend> {
        let conns = self.conns.lock();
        self.backends
            .lock()
            .iter()
            .map(|b| Backend {
                conns: conns.get(&b.addr).copied().unwrap_or(0),
                ..*b
            })
            .collect()
    }

    pub(super) fn open(&self, addr: SocketAddr) {
        *self.conns.lock().entry(addr).or_insert(0) += 1;
    }

    pub(super) fn close(&self, addr: SocketAddr) {
        let mut conns = self.conns.lock();
        if let Some(n) = conns.get_mut(&addr) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                conns.remove(&addr);
            }
        }
    }
}
