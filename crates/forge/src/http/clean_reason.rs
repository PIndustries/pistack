// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::{ApiError, ApiResult};

pub(crate) fn clean_reason(reason: Option<String>) -> ApiResult<Option<String>> {
    let reason = reason
        .map(|r| r.trim().to_owned())
        .filter(|r| !r.is_empty());
    if reason.as_ref().is_some_and(|r| r.chars().count() > 256) {
        return Err(ApiError::bad_request(
            "reason must be at most 256 characters",
        ));
    }
    Ok(reason)
}
