// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use ipnet::IpNet;
use pistack_types::{ApiError, ApiResult};

use super::first_free::first_free;
use super::inside::inside;
use super::joined::joined;
use super::meets::meets;

/// The next free block of `prefix` addresses: the first block of that size,
/// aligned as a router would want it, that lies wholly inside one of `cidrs`
/// and shares no address with `used`. Blocks are tried in the order given, and
/// a free block never straddles two of them.
pub(crate) fn next_block(cidrs: &[IpNet], used: &[IpNet], prefix: u8) -> ApiResult<IpNet> {
    let prefix = u16::from(prefix);
    if cidrs.is_empty() {
        return Err(ApiError::bad_request(
            "the vpc has no cidr to take a block from",
        ));
    }
    for b in cidrs {
        let max = match b {
            IpNet::V4(_) => 32u16,
            IpNet::V6(_) => 128,
        };
        if prefix > max {
            return Err(ApiError::bad_request(format!(
                "/{prefix} does not fit the family of {b}"
            )));
        }
        if prefix < u16::from(b.prefix_len()) {
            return Err(ApiError::bad_request(format!(
                "/{prefix} is larger than {b}"
            )));
        }
        if !used.iter().all(|u| inside(*b, *u) || !meets(*b, *u)) {
            return Err(ApiError::internal(format!(
                "subnet {used:?} is partly inside {b}"
            )));
        }
    }
    // A sub-block of a block that starts on its own boundary is aligned to the
    // size asked for as well, so the walk starts at the block itself.
    let used: Vec<IpNet> = used.to_vec();
    for b in cidrs {
        let held: Vec<IpNet> = used.iter().copied().filter(|u| meets(*b, *u)).collect();
        if let Some(cand) = first_free(*b, &held, prefix) {
            return Ok(cand);
        }
    }
    Err(ApiError::conflict(format!(
        "the vpc has no free /{prefix} in {}",
        joined(cidrs)
    )))
}
