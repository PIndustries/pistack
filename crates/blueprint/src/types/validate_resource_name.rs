// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `validate_resource_name`: the shared name rule for specs and workflows.

use pistack_types::{ApiError, ApiResult};

pub(crate) fn validate_resource_name(name: &str) -> ApiResult<()> {
    if name.is_empty() || name.len() > 128 || name.contains('/') {
        return Err(ApiError::bad_request(
            "spec.name must be 1-128 characters without '/'",
        ));
    }
    Ok(())
}
