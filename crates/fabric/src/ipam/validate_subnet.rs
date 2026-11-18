// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::SubnetSpec;
use pistack_types::{ApiError, ApiResult};

use super::ip_to_u128::ip_to_u128;

/// Reject a subnet spec whose pools or gateway do not fit its CIDR.
pub fn validate_subnet(spec: &SubnetSpec) -> ApiResult<()> {
    if let Some(gw) = spec.gateway {
        if !spec.cidr.contains(&gw) {
            return Err(ApiError::bad_request(format!(
                "gateway {gw} is outside {}",
                spec.cidr
            )));
        }
    }
    for p in &spec.allocation_pools {
        if p.start.is_ipv4() != spec.cidr.addr().is_ipv4() || p.end.is_ipv4() != p.start.is_ipv4() {
            return Err(ApiError::bad_request(
                "allocation pool family does not match cidr",
            ));
        }
        if !spec.cidr.contains(&p.start) || !spec.cidr.contains(&p.end) {
            return Err(ApiError::bad_request(format!(
                "allocation pool {}-{} is outside {}",
                p.start, p.end, spec.cidr
            )));
        }
        if ip_to_u128(p.start) > ip_to_u128(p.end) {
            return Err(ApiError::bad_request(format!(
                "allocation pool start {} is after end {}",
                p.start, p.end
            )));
        }
    }
    for r in &spec.host_routes {
        if r.nexthop.is_ipv4() != spec.cidr.addr().is_ipv4() {
            return Err(ApiError::bad_request(
                "host route nexthop family does not match cidr",
            ));
        }
    }
    Ok(())
}
