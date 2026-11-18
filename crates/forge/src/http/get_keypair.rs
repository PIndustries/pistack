// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::Keypair;
use axum::{
    extract::{Path, State},
    Json,
};
use pistack_types::ApiResult;

use super::app_state::AppState;
use super::load_keypair::load_keypair;
use super::scope::Scope;

pub(crate) async fn get_keypair(
    State(s): State<AppState>,
    scope: Scope,
    Path(id): Path<String>,
) -> ApiResult<Json<Keypair>> {
    Ok(Json(load_keypair(&s, &scope, &id)?))
}
