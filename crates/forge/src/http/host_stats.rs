// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use axum::{
    extract::{Path, State},
    Json,
};
use pistack_types::ApiResult;

use super::app_state::AppState;
use super::hosts::hosts;
use super::scope::Scope;

/// What the host is using now, as its agent last reported it.
pub(crate) async fn host_stats(
    State(s): State<AppState>,
    scope: Scope,
    Path(id): Path<String>,
) -> ApiResult<Json<crate::stats::Stats>> {
    scope.require_admin()?;
    let host = hosts(&s).resolve(None, &id)?;
    let gauge = gauge::Client::new(s.store.clone(), s.samples.clone());
    Ok(Json(crate::stats::host_stats(&gauge, &host)))
}
