// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::AlarmSpec;
use pistack_types::{ApiError, ApiResult};

use super::validate_name::validate_name;

pub(crate) fn validate_alarm(spec: &AlarmSpec) -> ApiResult<()> {
    validate_name(&spec.name)?;
    if spec.metric.trim().is_empty() || spec.metric.len() > 256 {
        return Err(ApiError::bad_request("metric must be 1-256 characters"));
    }
    if !spec.threshold.is_finite() {
        return Err(ApiError::bad_request("threshold must be finite"));
    }
    Ok(())
}
