// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `source_ip`: one client address, one backend, for as long as the
//! backend set holds still. The ring the hash walks, the pins remembered
//! against it, and the hash itself.

use super::backend::Backend;
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

/// FNV-1a over the address bytes. Fixed so a restart, or another server
/// in the cluster, pins the same client to the same slot.
pub(super) fn hash_ip(ip: IpAddr) -> u64 {
    let mut bytes: Vec<u8> = Vec::new();
    match ip {
        IpAddr::V4(a) => bytes.extend_from_slice(&a.octets()),
        IpAddr::V6(a) => bytes.extend_from_slice(&a.octets()),
    }
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

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
