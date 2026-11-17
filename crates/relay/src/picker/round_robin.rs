// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Smooth weighted round robin: the rotation of what each backend is
//! still owed, and the choice that settles one round at a time.

use super::backend::Backend;
use std::net::SocketAddr;

/// What each backend is still owed, in pool order.
#[derive(Debug, Default)]
pub(super) struct Rotation {
    /// What each backend is still owed, in pool order.
    pub(super) due: Vec<(SocketAddr, i64)>,
}

impl Rotation {
    /// Smooth weighted round robin: each round every backend is credited
    /// its weight, and the one owing the most takes the next connection
    /// and pays a round back. A backend of weight 2 is back after one turn
    /// by the others and never twice in a row, and the shares come out
    /// exactly. What is still owed when a member joins or leaves goes with
    /// it, so one change moves one connection and no more.
    pub(super) fn pick(&mut self, live: &[&Backend]) -> Option<SocketAddr> {
        let total: u32 = live.iter().map(|b| u32::from(b.weight)).sum();
        if total == 0 {
            return None;
        }
        self.due
            .retain(|(addr, _)| live.iter().any(|b| b.addr == *addr && b.weight > 0));
        for b in live {
            if !self.due.iter().any(|(addr, _)| *addr == b.addr) {
                self.due.push((b.addr, 0));
            }
        }
        for (addr, owed) in self.due.iter_mut() {
            *owed += live
                .iter()
                .find(|b| b.addr == *addr)
                .map_or(0, |b| i64::from(b.weight));
        }
        let mut at = 0;
        for (i, (_, owed)) in self.due.iter().enumerate() {
            if owed > &self.due[at].1 {
                at = i;
            }
        }
        self.due[at].1 -= i64::from(total);
        let chosen = self.due[at].0;
        Some(chosen)
    }
}
