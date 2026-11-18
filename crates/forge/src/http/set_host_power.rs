// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use axum::{
    extract::{Path, State},
    Json,
};
use pistack_types::ApiResult;
use serde::Deserialize;

use super::app_state::AppState;
use super::hosts::hosts;
use super::power_view::power_view;
use super::scope::Scope;

/// Body of `PUT /hosts/{id}/power`. The password is never here: `secret`
/// names a secret in the secrets service that holds it.
#[derive(Debug, Deserialize)]
pub struct SetHostPower {
    pub kind: String,
    pub address: String,
    pub user: String,
    pub secret: String,
}

pub(crate) async fn set_host_power(
    State(s): State<AppState>,
    scope: Scope,
    Path(id): Path<String>,
    Json(body): Json<SetHostPower>,
) -> ApiResult<Json<serde_json::Value>> {
    scope.require_admin()?;
    let host = hosts(&s).resolve(None, &id)?;
    let pc =
        crate::types::PowerControl::checked(&body.kind, &body.address, &body.user, &body.secret)?;
    let host = hosts(&s).modify(&host.meta.id, |h| {
        h.spec.power_control = Some(pc.clone());
        Ok(())
    })?;
    Ok(Json(power_view(&host)))
}
