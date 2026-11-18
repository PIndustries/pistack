// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::SubnetSpec;
use pistack_types::{ApiError, ApiResult};
use std::collections::BTreeSet;
use std::net::IpAddr;

use super::ip_to_u128::ip_to_u128;
use super::is_reserved::is_reserved;
use super::next_ip::next_ip;
use super::pools::pools;

/// Allocate the lowest free address in the subnet's pools.
pub fn allocate(spec: &SubnetSpec, used: &BTreeSet<IpAddr>) -> ApiResult<IpAddr> {
    for pool in pools(spec) {
        let mut cur = Some(pool.start);
        let end = ip_to_u128(pool.end);
        while let Some(ip) = cur {
            if ip_to_u128(ip) > end {
                break;
            }
            if !is_reserved(spec, ip) && !used.contains(&ip) {
                return Ok(ip);
            }
            cur = next_ip(ip);
        }
    }
    Err(ApiError::conflict("subnet full").with_detail("cidr", spec.cidr.to_string()))
}
