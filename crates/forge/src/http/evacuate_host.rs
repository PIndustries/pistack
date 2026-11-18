// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use axum::{
    extract::{Path, State},
    Json,
};
use pistack_types::ApiResult;

use super::app_state::AppState;
use super::hosts::hosts;
use super::optional_body::optional_body;
use super::scope::Scope;

/// Reschedule the machines of a host that is down. See
/// [`crate::maintain::evacuate`] for what is moved and what is lost.
pub(crate) async fn evacuate_host(
    State(s): State<AppState>,
    scope: Scope,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> ApiResult<Json<crate::maintain::EvacuateReport>> {
    scope.require_admin()?;
    let opts: crate::maintain::EvacuateOptions = optional_body(&body)?;
    let host = hosts(&s).resolve(None, &id)?;
    let out = crate::maintain::evacuate(&s.store, &host, opts, Some(s.fencer.as_ref())).await?;
    // What was asked of the host no longer applies. If it comes back it
    // must not be handed a start or an attach from before.
    if let Some(queue) = &s.queue {
        let dropped = queue.drop_commands(&host.spec.node_name);
        if dropped > 0 {
            tracing::info!(host = %host.meta.id, dropped, "forge: commands for the evacuated host dropped");
        }
    }
    Ok(Json(crate::maintain::EvacuateReport {
        host,
        proof: out.proof,
        power_off_failed: out.power_off_failed,
        evacuated: out.evacuated,
        skipped: out.skipped,
    }))
}
