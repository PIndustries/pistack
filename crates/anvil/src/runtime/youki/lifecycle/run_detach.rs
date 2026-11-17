// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// `create` / `start`. Stdout and stderr are files or `/dev/null`, opened
/// here so a sudo'd youki still writes through the agent's credentials.
/// youki's own errors are `--log` plus that stderr file, read after exit.
use crate::runtime::youki::bundles::path_arg;
use crate::runtime::youki::exec::timeout_label;
use crate::runtime::youki::exec::GroupKill;
use crate::runtime::youki::lifecycle::arm_user_ns_join;
use crate::runtime::youki::lifecycle::command_program;
use crate::runtime::youki::lifecycle::detach_stderr;
use crate::runtime::youki::lifecycle::open_append;
use crate::runtime::youki::lifecycle::stderr_tail;
use crate::runtime::youki::lifecycle::stdio_target;
use crate::runtime::youki::lifecycle::youki_command;
use crate::runtime::youki::lifecycle::DetachIo;
use crate::runtime::youki::privilege::RunMode;
use crate::runtime::youki::youki_timeout::youki_timeout;
use pistack_types::ApiError;
use pistack_types::ApiResult;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

pub(in crate::runtime::youki) async fn run_detach(
    bin: &Path,
    mode: &RunMode,
    args: &[&str],
    join_user_ns: Option<&Path>,
    io: &DetachIo,
) -> ApiResult<std::process::Output> {
    // Create the log as the agent. Root (sudo) then appends to an existing
    // file instead of making a root-owned one the agent cannot read.
    let _created = open_append(&io.youki_log)?;
    let mut cmd_args = Vec::with_capacity(args.len() + 2);
    cmd_args.push("--log".to_string());
    cmd_args.push(path_arg(&io.youki_log));
    cmd_args.extend(args.iter().copied().map(str::to_string));

    let mut cmd = youki_command(bin, &mode.privilege);
    cmd.args(&cmd_args)
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(stdio_target(io.stdout.as_deref())?)
        .stderr(stdio_target(io.stderr.as_deref())?);
    #[cfg(unix)]
    cmd.process_group(0);
    arm_user_ns_join(&mut cmd, join_user_ns);
    let program = command_program(bin, mode);
    let mut child = crate::spawn_retry::spawn_retrying_busy(|| cmd.spawn())
        .await
        .map_err(|e| ApiError::unavailable(format!("youki runtime: cannot run {program}: {e}")))?;
    let mut group = GroupKill::arm(child.id());
    let timeout = youki_timeout();
    let status = match tokio::time::timeout(timeout, child.wait()).await {
        Ok(Ok(status)) => {
            group.disarm();
            status
        }
        Ok(Err(e)) => {
            drop(group);
            let _ = child.start_kill();
            let _ = tokio::time::timeout(Duration::from_secs(2), child.wait()).await;
            return Err(ApiError::unavailable(format!(
                "youki runtime: {}: {e}",
                bin.display()
            )));
        }
        Err(_) => {
            drop(group);
            let _ = child.start_kill();
            let _ = tokio::time::timeout(Duration::from_secs(2), child.wait()).await;
            let op = args.first().copied().unwrap_or("youki");
            return Err(ApiError::unavailable(format!(
                "youki runtime: {op} timed out after {}",
                timeout_label(timeout)
            )));
        }
    };
    let output = std::process::Output {
        status,
        stdout: Vec::new(),
        stderr: detach_stderr(io),
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
