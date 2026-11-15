// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::privilege_hint::privilege_hint;
use super::sudo_program::sudo_program;
use super::want_sudo::want_sudo;
use pistack_types::{ApiError, ApiResult};
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;

pub(in crate::runtime::volfs) async fn exec(
    bin: &Path,
    args: &[String],
    timeout: Duration,
) -> ApiResult<std::process::Output> {
    let elevate = want_sudo();
    let program = if elevate {
        sudo_program()?
    } else {
        bin.to_path_buf()
    };
    let mut cmd = Command::new(&program);
    if elevate {
        cmd.arg("-n").arg(bin);
    }
    cmd.args(args)
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = cmd.spawn().map_err(|err| {
        ApiError::unavailable(format!(
            "{} is not installed ({}: {err}){}",
            bin.display(),
            program.display(),
            privilege_hint()
        ))
    })?;
    match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(Ok(output)) => Ok(output),
        Ok(Err(err)) => Err(ApiError::unavailable(format!(
            "{} failed: {err}{}",
            bin.display(),
            privilege_hint()
        ))),
        Err(_) => Err(ApiError::unavailable(format!(
            "{} timed out after {}s{}",
            bin.display(),
            timeout.as_secs(),
            privilege_hint()
        ))),
    }
}
