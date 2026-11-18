// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::Machine;
use axum::{
    extract::{Path, State},
    Json,
};
use pistack_types::ApiResult;

use super::app_state::AppState;
use super::power_action::power_action;
use super::scope::Scope;

pub(crate) async fn start_machine(
    State(s): State<AppState>,
    scope: Scope,
    Path(id): Path<String>,
) -> ApiResult<Json<Machine>> {
    power_action(&s, &scope, &id, |sp| {
        sp.power = crate::types::DesiredPower::On
    })
    .await
}
