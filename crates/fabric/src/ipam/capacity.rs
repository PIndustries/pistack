// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::SubnetSpec;

use super::broadcast::broadcast;
use super::ip_to_u128::ip_to_u128;
use super::pools::pools;

/// Total allocatable addresses in the pools (reserved ones excluded).
pub fn capacity(spec: &SubnetSpec) -> u128 {
    pools(spec)
        .iter()
        .map(|p| {
            let s = ip_to_u128(p.start);
            let e = ip_to_u128(p.end);
            let mut n = e.saturating_sub(s) + 1;
            for r in [
                Some(spec.cidr.network()),
                spec.gateway,
                broadcast(spec.cidr),
            ]
            .into_iter()
            .flatten()
            {
                let x = ip_to_u128(r);
                if x >= s && x <= e {
                    n = n.saturating_sub(1);
                }
            }
            n
        })
        .sum()
}
