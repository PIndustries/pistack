// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::exec::drain_text;
use crate::runtime::youki::exec::exec_argv;
use crate::runtime::youki::exec::spawn_reader;
use crate::runtime::youki::exec::timeout_label;
use crate::runtime::youki::exec::GroupKill;
use crate::runtime::youki::lifecycle::youki_command;
use crate::runtime::youki::privilege::Privilege;
use pistack_rpc::proto::ExecOutput;
use pistack_types::ApiError;
use pistack_types::ApiResult;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

#[allow(clippy::too_many_arguments)]
pub(in crate::runtime::youki) async fn run_exec(
    bin: &Path,
    privilege: &Privilege,
    container_id: &str,
    argv: &[String],
    env: &[(String, String)],
    cwd: Option<&str>,
    timeout: Duration,
    output_cap: usize,
) -> ApiResult<ExecOutput> {
    let args = exec_argv(container_id, argv, env, cwd);
    let mut cmd = youki_command(bin, privilege);
    cmd.args(&args)
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    cmd.process_group(0);
    let program = match privilege {
        Privilege::Sudo { sudo } => sudo.display().to_string(),
        Privilege::Root | Privilege::Rootless { .. } => bin.display().to_string(),
    };
    let mut child = crate::spawn_retry::spawn_retrying_busy(|| cmd.spawn())
        .await
        .map_err(|e| ApiError::unavailable(format!("youki runtime: cannot run {program}: {e}")))?;
    let mut group = GroupKill::arm(child.id());
    let stdout = spawn_reader(child.stdout.take(), output_cap);
    let stderr = spawn_reader(child.stderr.take(), output_cap);
    let status = match tokio::time::timeout(timeout, child.wait()).await {
        Ok(Ok(status)) => {
            group.disarm();
            status
        }
        Ok(Err(e)) => {
            drop(group);
            let _ = child.start_kill();
            let _ = tokio::time::timeout(Duration::from_secs(2), child.wait()).await;
            let _ = drain_text(stdout).await;
            let _ = drain_text(stderr).await;
            return Err(ApiError::unavailable(format!(
                "youki runtime: {}: {e}",
                bin.display()
            )));
        }
        Err(_) => {
            drop(group);
            let _ = child.start_kill();
            let _ = tokio::time::timeout(Duration::from_secs(2), child.wait()).await;
            let _ = drain_text(stdout).await;
            let _ = drain_text(stderr).await;
            return Err(ApiError::unavailable(format!(
                "youki runtime: exec timed out after {}",
                timeout_label(timeout)
            )));
        }
    };
    Ok(ExecOutput {
        exit_code: status.code().unwrap_or(1),
        stdout: drain_text(stdout).await,
        stderr: drain_text(stderr).await,
    })
}
