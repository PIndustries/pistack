// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Running the plan: start the publisher, or queue an edge publish, and
//! keep the one this process owns so a repeat apply is a no-op.

use super::binary::resolve_bin;
use super::edge::apply_edge;
use super::iface::refuse_wg_args;
use super::kind::{DoorKind, DoorSide};
use super::plan::{plan, DoorPlan};
use super::process::stop;
use super::process::{doors, stop_process, Slot};
use super::wireguard::try_create_pw;
use pistack_types::ApiError;
use std::time::Duration;

/// Run [`plan`] for `id`. An empty argv is success and starts nothing,
/// except an edge door, which queues a publish or stays unavailable.
/// A missing cloudflared, tailscale, or wg binary is unavailable. Tailscale
/// `serve --bg` and `wg set` must exit 0. Cloudflared is kept running.
pub async fn apply(
    id: &str,
    kind: DoorKind,
    listen: &str,
    upstream: &str,
    side: DoorSide<'_>,
) -> Result<DoorPlan, ApiError> {
    let plan = plan(kind, id, listen, upstream)?;
    if kind == DoorKind::Edge {
        return apply_edge(id, listen, upstream, plan, side).await;
    }
    if plan.bin.is_empty() {
        stop(id);
        return Ok(plan);
    }
    let argv = plan.argv.clone();
    {
        let mut map = doors().lock();
        if let Some(slot) = map.get_mut(id) {
            if slot.argv == argv {
                if slot.one_shot {
                    return Ok(plan);
                }
                if let Some(child) = slot.child.as_mut() {
                    if let Ok(None) = child.try_wait() {
                        return Ok(plan);
                    }
                }
            }
        }
    }
    stop_process(id);
    let bin = resolve_bin(kind, &plan.bin)?;
    if kind == DoorKind::Wireguard {
        refuse_wg_args(&plan.argv)?;
        let iface = &plan.argv[1];
        try_create_pw(iface).await;
    }
    let mut cmd = tokio::process::Command::new(&bin);
    cmd.args(&argv)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    let mut child = crate::spawn_retry::spawn_retrying_busy(|| cmd.spawn())
        .await
        .map_err(|e| ApiError::unavailable(format!("{} failed to start: {e}", plan.bin)))?;
    match kind {
        DoorKind::Tailscale | DoorKind::Wireguard => {
            let status = match tokio::time::timeout(Duration::from_secs(15), child.wait()).await {
                Ok(Ok(status)) => status,
                Ok(Err(e)) => {
                    return Err(ApiError::unavailable(format!(
                        "{} failed to start: {e}",
                        plan.bin
                    )));
                }
                Err(_) => {
                    let _ = child.start_kill();
                    return Err(ApiError::unavailable(format!("{} timed out", plan.bin)));
                }
            };
            if !status.success() {
                return Err(ApiError::unavailable(format!(
                    "{} exited {}",
                    plan.bin,
                    status.code().unwrap_or(-1)
                )));
            }
            doors().lock().insert(
                id.to_owned(),
                Slot {
                    argv,
                    child: None,
                    one_shot: true,
                },
            );
        }
        DoorKind::Cloudflared => {
            match child.try_wait() {
                Ok(Some(status)) => {
                    return Err(ApiError::unavailable(format!(
                        "cloudflared exited {}",
                        status.code().unwrap_or(-1)
                    )));
                }
                Ok(None) => {}
                Err(e) => {
                    return Err(ApiError::unavailable(format!(
                        "cloudflared failed to start: {e}"
                    )));
                }
            }
            doors().lock().insert(
                id.to_owned(),
                Slot {
                    argv,
                    child: Some(child),
                    one_shot: false,
                },
            );
        }
        DoorKind::Edge => {}
    }
    Ok(plan)
}
