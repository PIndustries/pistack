// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::ipam;
use crate::types::*;
use pistack_types::{ApiError, ApiResult};
/// Allowed address pairs are the exceptions to a port's own filter, so they
/// need a filter to be in. A whole-family prefix is refused too: it lets the
/// machine claim any address there.
pub(crate) fn validate_pairs(spec: &PortSpec) -> ApiResult<()> {
    if spec.allowed_address_pairs.is_empty() {
        return Ok(());
    }
    if !spec.port_security {
        return Err(ApiError::bad_request(
            "allowed_address_pairs needs port security on this port",
        ));
    }
    ipam::validate_pairs(&spec.allowed_address_pairs)
}
