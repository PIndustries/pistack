// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use axum::{
    extract::{Path, State},
    Json,
};
use pistack_types::ApiResult;

use super::app_state::AppState;
use super::clean_reason::clean_reason;
use super::disable_host::DisableHost;
use super::hosts::hosts;
use super::optional_body::optional_body;
use super::scope::Scope;
use super::set_disabled::set_disabled;

/// Disable the host, then live-migrate what can be live-migrated. What
/// cannot is reported and left running. Calling it again is how an operator
/// sees what is still there.
pub(crate) async fn drain_host(
    State(s): State<AppState>,
    scope: Scope,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> ApiResult<Json<crate::maintain::DrainReport>> {
    scope.require_admin()?;
    let body: DisableHost = optional_body(&body)?;
    let host = hosts(&s).resolve(None, &id)?;
    let reason =
        clean_reason(body.reason)?.or_else(|| (!host.spec.disabled).then(|| "draining".to_owned()));
    let host = set_disabled(&s, &host.meta.id, reason)?;
    let (migrating, not_moved) =
        crate::maintain::drain(&s.store, s.queue.as_ref(), s.strategy, &host)?;
    Ok(Json(crate::maintain::DrainReport {
        host,
        migrating,
        not_moved,
    }))
}
