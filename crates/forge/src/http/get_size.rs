// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::Size;
use axum::{
    extract::{Path, State},
    Json,
};
use pistack_types::ApiResult;

use super::app_state::AppState;
use super::scope::Scope;
use super::sizes::sizes;

pub(crate) async fn get_size(
    State(s): State<AppState>,
    _scope: Scope,
    Path(id): Path<String>,
) -> ApiResult<Json<Size>> {
    Ok(Json(sizes(&s).resolve(None, &id)?))
}
