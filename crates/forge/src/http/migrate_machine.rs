// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::{Machine, DELETE_ANNOTATION};
use axum::{
    extract::{Path, State},
    Json,
};
use pistack_types::{ApiError, ApiResult};

use super::app_state::AppState;
use super::body::Body;
use super::hosts::hosts;
use super::load_machine::load_machine;
use super::migrate_body::MigrateBody;
use super::scope::Scope;

pub(crate) async fn migrate_machine(
    State(s): State<AppState>,
    scope: Scope,
    Path(id): Path<String>,
    Body(body): Body<MigrateBody>,
) -> ApiResult<Json<Machine>> {
    let m = load_machine(&s, &scope, &id)?;
    if m.meta.annotations.contains_key(DELETE_ANNOTATION) {
        return Err(ApiError::conflict("machine is being deleted"));
    }
    let dest = hosts(&s).resolve(None, &body.host)?;
    let queue = s
        .queue
        .clone()
        .ok_or_else(|| ApiError::unavailable("live migration needs the agent transport"))?;
    // Host placement is updated in `begin_migration` after the command is
    // queued, not here.
    let m = crate::agent_driver::begin_migration(&s.store, &queue, &m, &dest)?;
    Ok(Json(m))
}
