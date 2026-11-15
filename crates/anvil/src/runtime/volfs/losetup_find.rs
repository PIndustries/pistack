// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::command_failed::command_failed;
use super::consts::{LOSETUP_ENV, PROBE_TIMEOUT};
use super::exec::exec;
use super::loop_device_ok::loop_device_ok;
use super::losetup_find_args::losetup_find_args;
use super::tool_program::tool_program;
use pistack_types::{ApiError, ApiResult};

pub(in crate::runtime::volfs) async fn losetup_find(file: &str) -> ApiResult<String> {
    let bin = tool_program(LOSETUP_ENV, "losetup");
    let output = exec(&bin, &losetup_find_args(file), PROBE_TIMEOUT).await?;
    if !output.status.success() {
        return Err(command_failed("losetup", &output));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let device = text.split_whitespace().next().unwrap_or("").trim();
    if !loop_device_ok(device) {
        return Err(ApiError::unavailable(
            "losetup did not return a loop device",
        ));
    }
    Ok(device.to_owned())
}
