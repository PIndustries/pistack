// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::Keypair;
use axum::{extract::State, Json};
use pistack_types::{ApiResult, Page};

use super::app_state::AppState;
use super::keypairs::keypairs;
use super::list::List;
use super::paginate::paginate;
use super::scope::Scope;

pub(crate) async fn list_keypairs(
    State(s): State<AppState>,
    scope: Scope,
    List(q): List,
) -> ApiResult<Json<Page<Keypair>>> {
    Ok(Json(paginate(keypairs(&s).list(scope.filter())?, &q)))
}
