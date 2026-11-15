// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::command_failed::command_failed;
use super::consts::{MKFS_ENV, MKFS_TIMEOUT};
use super::exec::exec;
use super::mkfs_command::mkfs_command;
use super::tool_program::tool_program;
use pistack_types::ApiResult;

pub(in crate::runtime::volfs) async fn make_filesystem(fs: &str, device: &str) -> ApiResult<()> {
    let (program, args) = mkfs_command(fs, device)?;
    let bin = tool_program(MKFS_ENV, &program);
    let output = exec(&bin, &args, MKFS_TIMEOUT).await?;
    if output.status.success() {
        Ok(())
    } else {
        Err(command_failed(&program, &output))
    }
}
