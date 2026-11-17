// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::{ApiError, ApiResult};

pub(crate) fn require_name(kind_name: &str, name: &str) -> ApiResult<String> {
    let name = name.trim();
    if name.is_empty() {
        return Err(ApiError::bad_request(format!(
            "{kind_name} is missing a name"
        )));
    }
    Ok(name.to_owned())
}
