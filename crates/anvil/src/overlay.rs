// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Apply a fabric overlay plan on this host.
//!
//! The plan is the document `pistack-overlay` converges: one bridge and one
//! VXLAN device per segment, one network namespace per router. This module
//! does not invent topology. A missing helper is a failure, because a guest
//! must not boot onto a bridge that was never created.

use pistack_types::{ApiError, ApiResult};
use std::path::Path;
use std::time::Duration;
use tokio::process::Command;

/// Write `plan` and converge it. `PISTACK_OVERLAY_BIN` overrides the helper
/// path. The default is `/usr/local/sbin/pistack-overlay`.
pub async fn apply_plan(plan: &serde_json::Value) -> ApiResult<()> {
    let bin = std::env::var("PISTACK_OVERLAY_BIN")
        .unwrap_or_else(|_| "/usr/local/sbin/pistack-overlay".to_owned());
    if bin.trim().is_empty() || bin.contains('\0') {
        return Err(ApiError::internal("overlay helper path is not usable"));
    }
    if std::env::var_os("PISTACK_OVERLAY_BIN").is_none() && !Path::new(&bin).exists() {
        return Err(ApiError::unavailable(
            "pistack-overlay is not installed on this host",
        ));
    }
    let path = std::env::temp_dir().join(format!("pistack-overlay-{}.json", std::process::id()));
    let body = serde_json::to_vec_pretty(plan)
        .map_err(|e| ApiError::internal(format!("overlay plan: {e}")))?;
    tokio::fs::write(&path, &body)
        .await
        .map_err(|e| ApiError::internal(format!("writing overlay plan: {e}")))?;
    let run = Command::new("sudo")
        .args(["-n", &bin, "apply", "--plan"])
        .arg(&path)
        .kill_on_drop(true)
        .output();
    let outcome = tokio::time::timeout(Duration::from_secs(30), run).await;
    let _ = tokio::fs::remove_file(&path).await;
    match outcome {
        Ok(Ok(out)) if out.status.success() => Ok(()),
        Ok(Ok(out)) => {
            let err = String::from_utf8_lossy(&out.stderr);
            let msg = err.trim();
            Err(ApiError::unavailable(if msg.is_empty() {
                "overlay helper failed".into()
            } else {
                format!("overlay helper failed: {msg}")
            }))
        }
        Ok(Err(e)) => Err(ApiError::unavailable(format!("overlay helper did not run: {e}"))),
        Err(_) => Err(ApiError::unavailable("overlay helper timed out")),
    }
}
