// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::new_size;
use axum::{extract::State, response::Response};
use pistack_types::ApiResult;
use serde::Deserialize;

use super::app_state::AppState;
use super::body::Body;
use super::created::created;
use super::scope::Scope;
use super::sizes::sizes;
use super::validate_name::validate_name;
use super::validate_resources::validate_resources;

#[derive(Debug, Deserialize)]
pub struct CreateSize {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(flatten)]
    pub resources: pistack_types::Resources,
}

pub(crate) async fn create_size(
    State(s): State<AppState>,
    scope: Scope,
    Body(body): Body<CreateSize>,
) -> ApiResult<Response> {
    scope.require_admin()?;
    validate_name(&body.name)?;
    validate_resources(&body.resources)?;
    let mut sz = new_size(body.name, body.resources);
    sz.meta.description = body.description;
    Ok(created(sizes(&s).create(sz)?))
}
