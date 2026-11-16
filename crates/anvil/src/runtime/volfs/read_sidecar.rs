// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::io_err::io_err;
use super::loop_device_ok::loop_device_ok;
use pistack_types::{ApiError, ApiResult};
use std::path::Path;

pub(in crate::runtime::volfs) fn read_sidecar(path: &Path) -> ApiResult<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(text) => {
            let device = text.trim();
            if device.is_empty() {
                Ok(None)
            } else if loop_device_ok(device) {
                Ok(Some(device.to_owned()))
            } else {
                Err(ApiError::unavailable(format!(
                    "loop device {device} cannot be detached"
                )))
            }
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(io_err("reading", path, err)),
    }
}
