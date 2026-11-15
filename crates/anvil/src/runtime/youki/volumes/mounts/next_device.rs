// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::volumes::AttachedDisk;
use pistack_types::ApiError;
use pistack_types::ApiResult;

pub(in crate::runtime::youki) fn next_device(disks: &[AttachedDisk]) -> ApiResult<String> {
    for c in b'b'..=b'z' {
        let name = format!("vd{}", c as char);
        if disks.iter().all(|d| d.device != name) {
            return Ok(name);
        }
    }
    Err(ApiError::unprocessable("no free virtio device name"))
}
