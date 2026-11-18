// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::Host;
use axum::{
    extract::{Path, State},
    Json,
};
use pistack_types::ApiResult;
use serde::Deserialize;

use super::app_state::AppState;
use super::clean_reason::clean_reason;
use super::hosts::hosts;
use super::optional_body::optional_body;
use super::scope::Scope;
use super::set_disabled::set_disabled;

#[derive(Debug, Deserialize, Default)]
pub struct DisableHost {
    #[serde(default)]
    pub reason: Option<String>,
}

pub(crate) async fn disable_host(
    State(s): State<AppState>,
    scope: Scope,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> ApiResult<Json<Host>> {
    scope.require_admin()?;
    let body: DisableHost = optional_body(&body)?;
    let reason = clean_reason(body.reason)?;
    let host = hosts(&s).resolve(None, &id)?;
    Ok(Json(set_disabled(&s, &host.meta.id, reason)?))
}
