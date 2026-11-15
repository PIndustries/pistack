// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::command_failed::command_failed;
use super::consts::{MOUNT_ENV, PROBE_TIMEOUT};
use super::exec::exec;
use super::mount_args::mount_args;
use super::tool_program::tool_program;
use pistack_types::ApiResult;
use std::path::Path;

pub(in crate::runtime::volfs) async fn mount_device(
    device: &str,
    dir: &Path,
    read_only: bool,
) -> ApiResult<()> {
    let bin = tool_program(MOUNT_ENV, "mount");
    let dir = dir.to_string_lossy().into_owned();
    let output = exec(&bin, &mount_args(device, &dir, read_only), PROBE_TIMEOUT).await?;
    if output.status.success() {
        Ok(())
    } else {
        Err(command_failed("mount", &output))
    }
}
