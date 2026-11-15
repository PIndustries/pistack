// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::blkid_args::blkid_args;
use super::command_failed::command_failed;
use super::consts::{BLKID_ENV, PROBE_TIMEOUT};
use super::exec::exec;
use super::first_token::first_token;
use super::tool_program::tool_program;
use pistack_types::ApiResult;

pub(in crate::runtime::volfs) async fn filesystem_type(device: &str) -> ApiResult<Option<String>> {
    let bin = tool_program(BLKID_ENV, "blkid");
    let output = exec(&bin, &blkid_args(device), PROBE_TIMEOUT).await?;
    let found = first_token(&output.stdout);
    if output.status.success() {
        return Ok(found);
    }
    // blkid exits 2 when the device has no recognizable filesystem.
    if output.status.code() == Some(2) && found.is_none() {
        return Ok(None);
    }
    if found.is_some() {
        return Ok(found);
    }
    Err(command_failed("blkid", &output))
}
