// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::{ApiError, ApiResult, Id};

pub(in crate::runtime::volfs) fn volume_name(volume: &Id) -> ApiResult<String> {
    let name = volume.to_string();
    if !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        Ok(name)
    } else {
        Err(ApiError::bad_request(
            "volume id cannot name a mount directory",
        ))
    }
}
