// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::types::*;
use pistack_types::{ApiError, ApiResult};
pub(crate) fn validate_routes(routes: &[StaticRoute]) -> ApiResult<()> {
    for r in routes {
        if r.destination.addr().is_ipv4() != r.nexthop.is_ipv4() {
            return Err(ApiError::bad_request(format!(
                "route {} via {}: address family mismatch",
                r.destination, r.nexthop
            )));
        }
    }
    Ok(())
}
