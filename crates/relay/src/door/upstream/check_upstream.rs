// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Rejecting an `upstream` a door may not be told to publish.

use super::allowed_upstream::allowed_upstream;
use pistack_types::{ApiError, ApiResult};

/// `http://127.0.0.1:PORT` (port required) or `http://` plus an address in
/// 10/8, 192.168/16, or 172.16/12. No path, query, userinfo, or other host.
pub(crate) fn check_upstream(upstream: &str) -> ApiResult<()> {
    if allowed_upstream(upstream) {
        Ok(())
    } else {
        Err(ApiError::bad_request(
            "upstream must be http://127.0.0.1:PORT or http://10.*, http://192.168.*, or http://172.16-31.*",
        ))
    }
}
