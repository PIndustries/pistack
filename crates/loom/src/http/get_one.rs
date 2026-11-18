// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use axum::{
    extract::{Path, State},
    Json,
};
use pistack_types::ApiResult;

use crate::http::api::Api;
use crate::http::app_state::AppState;
use crate::http::load::load;
use crate::http::res::Res;
use crate::http::scope::Scope;

pub(crate) async fn get_one<A: Api>(
    State(s): State<AppState>,
    scope: Scope,
    Path(id): Path<String>,
) -> ApiResult<Json<Res<A>>> {
    Ok(Json(load::<A>(&s, &scope, &id)?))
}
