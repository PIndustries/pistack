// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use axum::{
    extract::{Path, State},
    Json,
};
use pistack_types::ApiResult;

use super::app_state::AppState;
use super::load_machine::load_machine;
use super::scope::Scope;

/// What the machine is using now, as its host last reported it. Anyone who
/// may see the machine may see this.
pub(crate) async fn machine_stats(
    State(s): State<AppState>,
    scope: Scope,
    Path(id): Path<String>,
) -> ApiResult<Json<crate::stats::Stats>> {
    let machine = load_machine(&s, &scope, &id)?;
    let gauge = gauge::Client::new(s.store.clone(), s.samples.clone());
    Ok(Json(crate::stats::machine_stats(&gauge, &machine)))
}
