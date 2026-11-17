// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `source_ip`: one client address, one backend, for as long as the
//! backend set holds still. The ring the hash walks and the pins remembered
//! against it. The hash itself is in [`super::hash_ip`].

use super::backend::Backend;
use super::hash_ip::hash_ip;
use std::net::{IpAddr, SocketAddr};

/// `source_ip`'s state: the backend set the table was built for, and the
/// address each seen client is pinned to.
#[derive(Debug, Default)]
pub(super) struct Sticky {
    pub(super) ring: Vec<SocketAddr>,
    pub(super) pinned: Vec<(IpAddr, SocketAddr)>,
}

/// How many client addresses `source_ip` remembers. Past this, a client
/// is placed by its hash again rather than by its history.
const PINNED_CAP: usize = 4096;

impl Sticky {
    /// The client's address picks a slot in the backend set, so a set
    /// that loses a member moves as few clients as possible. The pin is
    /// remembered while the set holds still, so a client whose backend
    /// was passed over by the hash still lands on it every time.
    pub(super) fn pick(&mut self, client: IpAddr, live: &[&Backend]) -> Option<SocketAddr> {
        let mut ring: Vec<SocketAddr> = live.iter().map(|b| b.addr).collect();
        ring.sort_unstable();
        if self.ring != ring {
            self.pinned.retain(|(_, addr)| ring.contains(addr));
            self.ring.clone_from(&ring);
        }
        if let Some((_, addr)) = self.pinned.iter().find(|(ip, _)| *ip == client) {
            return Some(*addr);
        }
        let addr = self.ring[(hash_ip(client) as usize) % self.ring.len()];
        if self.pinned.len() < PINNED_CAP {
            self.pinned.push((client, addr));
        }
        Some(addr)
    }
}
