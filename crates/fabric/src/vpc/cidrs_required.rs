// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::VpcSpec;
use pistack_types::{ApiError, ApiResult};

/// A VPC spec is never sent without blocks.
pub(crate) fn cidrs_required(spec: &VpcSpec) -> ApiResult<()> {
    if spec.cidrs.is_empty() {
        return Err(ApiError::bad_request("spec.cidrs needs at least one block"));
    }
    Ok(())
}
