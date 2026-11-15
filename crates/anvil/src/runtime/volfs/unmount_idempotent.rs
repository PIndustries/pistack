// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::already_unmounted::already_unmounted;
use super::command_failed::command_failed;
use super::consts::{PROBE_TIMEOUT, UMOUNT_ENV};
use super::exec::exec;
use super::tool_program::tool_program;
use super::umount_args::umount_args;
use pistack_types::ApiResult;
use std::path::Path;

pub(in crate::runtime::volfs) async fn unmount_idempotent(dir: &Path) -> ApiResult<()> {
    let bin = tool_program(UMOUNT_ENV, "umount");
    let output = exec(&bin, &umount_args(&dir.to_string_lossy()), PROBE_TIMEOUT).await?;
    if output.status.success() || already_unmounted(&output) {
        return Ok(());
    }
    Err(command_failed("umount", &output))
}
