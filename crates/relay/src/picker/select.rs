// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The picker a pool holds: the algorithm in force, the state each
//! algorithm keeps, and the one `pick` that dispatches between them.

use super::backend::Backend;
use super::least_conn::least_conn;
use super::round_robin::Rotation;
use super::source_ip::Sticky;
use crate::types::Algorithm;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

/// Member selection for one pool. Cloning one hands the same rotation and
/// the same pins to the copy, so a data plane can give every connection
/// its own handle and still balance across all of them.
#[derive(Debug, Default)]
pub struct Picker {
    algo: Arc<std::sync::atomic::AtomicU8>,
    state: Arc<parking_lot::Mutex<Rotation>>,
    source: Arc<parking_lot::Mutex<Sticky>>,
}

impl Clone for Picker {
    fn clone(&self) -> Self {
        Self {
            algo: self.algo.clone(),
            state: self.state.clone(),
            source: self.source.clone(),
        }
    }
}

/// Kept private to this module so a bad name cannot reach [`Picker::new`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Algo {
    RoundRobin,
    LeastConn,
    SourceIp,
}

impl Picker {
    pub fn new(algo: Algorithm) -> Self {
        Self {
            algo: Arc::new(std::sync::atomic::AtomicU8::new(algo.code())),
            state: Arc::new(parking_lot::Mutex::new(Rotation::default())),
            source: Arc::new(parking_lot::Mutex::new(Sticky::default())),
        }
    }

    pub fn algorithm(&self) -> Algorithm {
        Algorithm::from_code(self.algo.load(std::sync::atomic::Ordering::Relaxed))
    }

    /// Switch algorithm, keeping what the current one has accumulated. A
    /// pool re-pointed at another algorithm starts counting from where it
    /// stands rather than from the top of a rotation.
    pub fn set_algorithm(&self, algo: Algorithm) {
        self.algo
            .store(algo.code(), std::sync::atomic::Ordering::Relaxed);
    }

    /// The backend this request goes to, `None` when nothing may take it.
    /// `client` is the peer address; only `source_ip` needs it, and
    /// without one the request goes round robin.
    pub fn pick(&self, backends: &[Backend], client: Option<IpAddr>) -> Option<SocketAddr> {
        let live: Vec<&Backend> = backends.iter().filter(|b| b.weight > 0).collect();
        if live.is_empty() {
            return None;
        }
        match self.algorithm().algo() {
            Algo::LeastConn => least_conn(&live),
            Algo::SourceIp => match client {
                Some(ip) => self.sticky(ip, &live),
                None => self.round_robin(&live),
            },
            Algo::RoundRobin => self.round_robin(&live),
        }
    }

    fn round_robin(&self, live: &[&Backend]) -> Option<SocketAddr> {
        self.state.lock().pick(live)
    }

    fn sticky(&self, client: IpAddr, live: &[&Backend]) -> Option<SocketAddr> {
        self.source.lock().pick(client, live)
    }
}
