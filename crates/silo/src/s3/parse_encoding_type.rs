// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::{ApiError, ApiResult};

/// `None`/empty → no encoding; `url` → encode; anything else → 400.
pub(crate) fn parse_encoding_type(raw: Option<&str>) -> ApiResult<bool> {
    match raw {
        None | Some("") => Ok(false),
        Some("url") => Ok(true),
        Some(other) => Err(ApiError::bad_request(format!(
            "encoding-type must be url, got '{other}'"
        ))),
    }
}
