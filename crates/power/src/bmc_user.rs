// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::{ApiError, ApiResult};

/// A BMC user, checked so it is safe to pass to a tool as one argument.
pub(crate) fn bmc_user(raw: &str) -> ApiResult<String> {
    let raw = raw.trim();
    if raw.is_empty()
        || raw.len() > 32
        || raw.starts_with('-')
        || !raw
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    {
        return Err(ApiError::bad_request("bmc user is not a name"));
    }
    Ok(raw.to_owned())
}
