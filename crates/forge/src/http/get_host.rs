// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::Host;
use axum::{
    extract::{Path, State},
    Json,
};
use pistack_types::ApiResult;

use super::app_state::AppState;
use super::hosts::hosts;
use super::scope::Scope;
use super::shown::shown;

pub(crate) async fn get_host(
    State(s): State<AppState>,
    scope: Scope,
    Path(id): Path<String>,
) -> ApiResult<Json<Host>> {
    scope.require_admin()?;
    Ok(Json(shown(&s, hosts(&s).resolve(None, &id)?)))
}
