// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The one name rule every write passes through.

use pistack_types::{ApiError, ApiResult};

pub(super) fn check_name(name: &str) -> ApiResult<&str> {
    let n = name.trim();
    if n.is_empty() || n.len() > 128 || n.contains('/') {
        return Err(ApiError::bad_request(
            "name must be 1-128 characters without '/'",
        ));
    }
    Ok(n)
}
