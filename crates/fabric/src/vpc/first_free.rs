// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use ipnet::IpNet;

use super::align::align;
use super::block_at::block_at;
use super::index::index;
use super::overlaps::overlaps;
use super::span::span;

/// The first `prefix` block inside `big` that no member of `held` — each of
/// them wholly inside `big` — touches. Candidates step from the lowest
/// boundary at or above `big`'s base to the last block that still fits.
pub(crate) fn first_free(big: IpNet, held: &[IpNet], prefix: u16) -> Option<IpNet> {
    let width = match big {
        IpNet::V4(_) => 32u16,
        IpNet::V6(_) => 128,
    };
    let size = 1u128 << (width - prefix);
    let base = index(big.network());
    let top = base.checked_add(span(big))?;
    let mut at = align(base, width, prefix)?;
    if at < base {
        at = at.checked_add(size)?;
    }
    while at.checked_add(size)? <= top {
        if let Some(cand) = block_at(big, at, prefix) {
            if !held.iter().any(|u| overlaps(cand, *u)) {
                return Some(cand);
            }
        }
        at = at.checked_add(size)?;
    }
    None
}
