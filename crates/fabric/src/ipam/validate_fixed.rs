// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::SubnetSpec;
use pistack_types::{ApiError, ApiResult};
use std::collections::BTreeSet;
use std::net::IpAddr;

use super::is_reserved::is_reserved;

/// Check that a caller-chosen address is usable in this subnet.
pub fn validate_fixed(spec: &SubnetSpec, ip: IpAddr, used: &BTreeSet<IpAddr>) -> ApiResult<()> {
    if !spec.cidr.contains(&ip) {
        return Err(ApiError::bad_request(format!(
            "{ip} is outside {}",
            spec.cidr
        )));
    }
    if is_reserved(spec, ip) {
        return Err(ApiError::bad_request(format!(
            "{ip} is reserved in {}",
            spec.cidr
        )));
    }
    if used.contains(&ip) {
        return Err(ApiError::conflict(format!("{ip} is already in use"))
            .with_detail("address", ip.to_string()));
    }
    Ok(())
}
