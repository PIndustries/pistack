// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::already_detached::already_detached;
use super::command_failed::command_failed;
use super::consts::{LOSETUP_ENV, PROBE_TIMEOUT};
use super::exec::exec;
use super::loop_device_ok::loop_device_ok;
use super::losetup_detach_args::losetup_detach_args;
use super::tool_program::tool_program;
use pistack_types::{ApiError, ApiResult};

pub(in crate::runtime::volfs) async fn detach_loop_idempotent(device: &str) -> ApiResult<()> {
    if !loop_device_ok(device) {
        return Err(ApiError::unavailable(format!(
            "loop device {device} cannot be detached"
        )));
    }
    let bin = tool_program(LOSETUP_ENV, "losetup");
    let output = exec(&bin, &losetup_detach_args(device), PROBE_TIMEOUT).await?;
    if output.status.success() || already_detached(&output) {
        return Ok(());
    }
    Err(command_failed("losetup", &output))
}
