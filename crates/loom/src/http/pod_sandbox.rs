// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use axum::{
    extract::{Path, State},
    Json,
};
use pistack_types::ApiResult;

use crate::http::app_state::AppState;
use crate::http::load::load;
use crate::http::scope::Scope;
use crate::types::PodSpec;

/// The document the pod's machine carries to anvil; handy for debugging.
pub(crate) async fn pod_sandbox(
    State(s): State<AppState>,
    scope: Scope,
    Path(id): Path<String>,
) -> ApiResult<Json<crate::controllers::pod::PodSandbox>> {
    let p = load::<PodSpec>(&s, &scope, &id)?;
    Ok(Json(crate::controllers::pod::PodSandbox::from_pod(&p)))
}
