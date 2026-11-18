// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::Size;
use axum::{
    extract::{Path, State},
    http::HeaderMap,
    Json,
};
use pistack_types::ApiResult;

use super::app_state::AppState;
use super::body::Body;
use super::create_size::CreateSize;
use super::if_match::if_match;
use super::scope::Scope;
use super::sizes::sizes;
use super::validate_name::validate_name;
use super::validate_resources::validate_resources;

pub(crate) async fn update_size(
    State(s): State<AppState>,
    scope: Scope,
    Path(id): Path<String>,
    headers: HeaderMap,
    Body(body): Body<CreateSize>,
) -> ApiResult<Json<Size>> {
    scope.require_admin()?;
    let expected = if_match(&headers)?;
    validate_name(&body.name)?;
    validate_resources(&body.resources)?;
    let mut sz = sizes(&s).resolve(None, &id)?;
    sz.meta.name = body.name;
    sz.meta.description = body.description;
    sz.spec.resources = body.resources;
    Ok(Json(sizes(&s).update(sz, expected)?))
}
