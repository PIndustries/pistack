// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Member rules: the address and weight a pool will accept.

use crate::types::MemberSpec;
use pistack_types::{ApiError, ApiResult};

pub(super) fn validate_member(spec: &MemberSpec) -> ApiResult<()> {
    if spec.port == 0 {
        return Err(ApiError::bad_request("member port must be 1-65535"));
    }
    if spec.weight > 256 {
        return Err(ApiError::bad_request("member weight must be 0-256"));
    }
    if spec.address.is_unspecified() || spec.address.is_multicast() {
        return Err(ApiError::bad_request(format!(
            "{} is not a usable member address",
            spec.address
        )));
    }
    Ok(())
}
