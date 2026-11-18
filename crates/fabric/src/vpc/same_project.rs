// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::{ApiError, ApiResult, Id};

/// One project for the VPC, its networks and its subnets.
pub(crate) fn same_project(mine: &Option<Id>, theirs: &Option<Id>, what: &str) -> ApiResult<()> {
    match (mine, theirs) {
        (_, None) => Ok(()),
        (Some(a), Some(b)) if a == b => Ok(()),
        _ => Err(ApiError::forbidden(format!(
            "the {what} belongs to another project"
        ))),
    }
}
