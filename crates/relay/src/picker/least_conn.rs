// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Fewest open connections. Stateless: the counts arrive with the
//! backends, so the choice is one pass over them.

use super::backend::Backend;
use std::net::SocketAddr;

/// Fewest open connections; a tie goes to the heavier backend, and one
/// between equals goes to the earlier of the two.
pub(super) fn least_conn(live: &[&Backend]) -> Option<SocketAddr> {
    let mut best: Option<&Backend> = None;
    for b in live {
        let better = match best {
            None => true,
            Some(have) => b.conns < have.conns || (b.conns == have.conns && b.weight > have.weight),
        };
        if better {
            best = Some(b);
        }
    }
    best.map(|b| b.addr)
}
