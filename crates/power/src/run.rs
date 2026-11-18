// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::{ApiError, ApiResult};
use std::path::Path;

use crate::clip::clip;
use crate::spawn_retry::output_retrying_busy;

pub(crate) async fn run(bin: &Path, args: &[String], what: &str) -> ApiResult<String> {
    let mut cmd = tokio::process::Command::new(bin);
    cmd.args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let output = output_retrying_busy(&mut cmd).await.map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            ApiError::unavailable(format!("{} is not installed", bin.display()))
        } else {
            ApiError::unavailable(format!("{} failed to start: {e}", bin.display()))
        }
    })?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    if !output.status.success() {
        let detail = clip(&String::from_utf8_lossy(&output.stderr));
        return Err(if detail.is_empty() {
            ApiError::unavailable(format!("{what} failed"))
        } else {
            ApiError::unavailable(format!("{what} failed: {detail}"))
        });
    }
    Ok(stdout)
}
