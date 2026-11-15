// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::ApiError;
use pistack_types::ApiResult;

pub(in crate::runtime::youki) fn check_device_name(name: &str) -> ApiResult<()> {
    let b = name.as_bytes();
    if b.len() == 3 && b[0] == b'v' && b[1] == b'd' && (b'b'..=b'z').contains(&b[2]) {
        return Ok(());
    }
    Err(ApiError::bad_request(
        "device must be a virtio name from vdb to vdz",
    ))
}
