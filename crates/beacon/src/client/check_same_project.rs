// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::{ApiError, ApiResult, Meta};

/// A project resource may only reference resources in its own project or
/// cloud-wide (project `None`) ones.
pub(crate) fn check_same_project(mine: &Meta, theirs: &Meta, what: &str) -> ApiResult<()> {
    match (&mine.project, &theirs.project) {
        (_, None) => Ok(()),
        (Some(a), Some(b)) if a == b => Ok(()),
        _ => Err(ApiError::forbidden(format!(
            "{what} {} belongs to another project",
            theirs.id
        ))),
    }
}
