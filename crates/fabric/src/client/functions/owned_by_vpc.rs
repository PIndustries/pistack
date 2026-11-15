// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::types::*;
use pistack_types::{ApiError, ApiResult};
/// A router a VPC owns is changed on the VPC, not on the router.
pub(crate) fn owned_by_vpc(r: &Router) -> ApiResult<()> {
    match &r.spec.vpc {
        // The name the router was given at birth, or its id when the vpc's own
        // row is already gone.
        Some(v) => Err(ApiError::conflict(format!(
            "router {} belongs to vpc {}; change it there",
            r.meta.id,
            crate::vpc::owner_name(r).unwrap_or_else(|| v.to_string())
        ))
        .with_detail("vpc", v.to_string())),
        None => Ok(()),
    }
}
