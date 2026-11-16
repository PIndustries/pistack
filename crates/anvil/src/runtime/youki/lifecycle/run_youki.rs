// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::exec::timeout_label;
use crate::runtime::youki::lifecycle::arm_user_ns_join;
use crate::runtime::youki::lifecycle::command_program;
use crate::runtime::youki::lifecycle::stderr_tail;
use crate::runtime::youki::lifecycle::youki_command;
use crate::runtime::youki::privilege::RunMode;
use crate::runtime::youki::youki_timeout::youki_timeout;
use pistack_types::ApiError;
use pistack_types::ApiResult;
use std::path::Path;
use std::process::Stdio;

pub(in crate::runtime::youki) async fn run_youki(
    bin: &Path,
    mode: &RunMode,
    args: &[&str],
    join_user_ns: Option<&Path>,
) -> ApiResult<std::process::Output> {
    let mut cmd = youki_command(bin, &mode.privilege);
    cmd.args(args)
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    arm_user_ns_join(&mut cmd, join_user_ns);
    let program = command_program(bin, mode);
    let child = cmd
        .spawn()
        .map_err(|e| ApiError::unavailable(format!("youki runtime: cannot run {program}: {e}")))?;
    let timeout = youki_timeout();
    let output = match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(Ok(output)) => output,
        Ok(Err(e)) => {
            return Err(ApiError::unavailable(format!(
                "youki runtime: {}: {e}",
                bin.display()
            )));
        }
        Err(_) => {
            let op = args.first().copied().unwrap_or("youki");
            return Err(ApiError::unavailable(format!(
                "youki runtime: {op} timed out after {}",
                timeout_label(timeout)
            )));
        }
    };
    if output.status.success() {
        return Ok(output);
    }
    let op = args.first().copied().unwrap_or("youki");
    Err(ApiError::unavailable(format!(
        "youki runtime: {op} failed: {}",
        stderr_tail(&output.stderr, output.status)
    )))
}
