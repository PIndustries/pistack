// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use axum::{
    extract::{Path, State},
    Json,
};
use pistack_types::ApiResult;

use super::app_state::AppState;
use super::hosts::hosts;
use super::power_view::power_view;
use super::scope::Scope;

pub(crate) async fn get_host_power(
    State(s): State<AppState>,
    scope: Scope,
    Path(id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    scope.require_admin()?;
    let host = hosts(&s).resolve(None, &id)?;
    Ok(Json(power_view(&host)))
}
