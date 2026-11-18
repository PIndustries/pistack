// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use ipnet::IpNet;
use pistack_types::{ApiError, ApiResult};

use super::overlaps::overlaps;

/// Blocks are given, and none of them overlaps another.
pub(crate) fn validate_cidrs(cidrs: &[IpNet]) -> ApiResult<()> {
    if cidrs.is_empty() {
        return Err(ApiError::bad_request("a vpc needs at least one cidr"));
    }
    for c in cidrs {
        if c.addr().is_unspecified() {
            return Err(ApiError::bad_request(format!("{c} is not a block")));
        }
    }
    for (i, a) in cidrs.iter().enumerate() {
        for b in &cidrs[i + 1..] {
            if overlaps(*a, *b) {
                return Err(ApiError::bad_request(format!("cidrs {a} and {b} overlap")));
            }
        }
    }
    Ok(())
}
