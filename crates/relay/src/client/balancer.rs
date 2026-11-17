// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The balancer's own rules: a usable vip and the fields that may not
//! change under a caller.

use crate::types::BalancerSpec;
use pistack_types::{ApiError, ApiResult};
use std::net::IpAddr;

pub(super) fn validate_balancer(spec: &BalancerSpec) -> ApiResult<()> {
    if spec.vip_network.trim().is_empty() {
        return Err(ApiError::bad_request("balancer needs a vip_network"));
    }
    if spec.vip_port == Some(0) {
        return Err(ApiError::bad_request("vip_port must be 1-65535"));
    }
    if let Some(IpAddr::V4(a)) = spec.vip_address {
        if a.is_unspecified() || a.is_broadcast() || a.is_multicast() {
            return Err(ApiError::bad_request(format!("{a} is not a usable vip")));
        }
    }
    if let Some(IpAddr::V6(a)) = spec.vip_address {
        if a.is_unspecified() || a.is_multicast() {
            return Err(ApiError::bad_request(format!("{a} is not a usable vip")));
        }
    }
    Ok(())
}
