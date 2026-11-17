// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `validate_workflow`: name plus the ordered stack list.

use pistack_types::{ApiError, ApiResult};

use super::validate_resource_name::validate_resource_name;
use super::workflow_spec::WorkflowSpec;

/// Name plus the ordered stack list. Each entry is trimmed.
pub fn validate_workflow(spec: &mut WorkflowSpec) -> ApiResult<()> {
    spec.name = spec.name.trim().to_owned();
    validate_resource_name(&spec.name)?;
    for name in &mut spec.stacks {
        *name = name.trim().to_owned();
        if name.is_empty() || name.len() > 128 || name.contains('/') {
            return Err(ApiError::bad_request(
                "workflow stack names must be 1-128 characters without '/'",
            ));
        }
    }
    Ok(())
}
