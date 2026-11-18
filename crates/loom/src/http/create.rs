// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::*;
use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use pistack_types::{ApiResult, Resource};
use serde::{de::DeserializeOwned, Deserialize};
use std::collections::BTreeMap;

use crate::http::api::Api;
use crate::http::app_state::AppState;
use crate::http::body::Body;
use crate::http::col::col;
use crate::http::res::Res;
use crate::http::scope::Scope;
use crate::http::validate_name::validate_name;

#[derive(Debug, Deserialize)]
#[serde(bound(deserialize = "S: DeserializeOwned"))]
pub struct Create<S> {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    #[serde(default)]
    pub annotations: BTreeMap<String, String>,
    pub spec: S,
}

pub(crate) async fn create<A: Api>(
    State(s): State<AppState>,
    scope: Scope,
    Body(body): Body<Create<A>>,
) -> ApiResult<Response> {
    validate_name(&body.name)?;
    let mut spec = body.spec;
    spec.validate(None)?;
    let mut r: Res<A> = Resource::new(body.name, scope.project.clone(), spec);
    r.meta.description = body.description;
    r.meta.labels = body.labels;
    r.meta.annotations = body.annotations;
    r.meta.annotations.remove(DELETE_ANNOTATION);
    let r = col::<A>(&s).create(r)?;
    Ok((StatusCode::CREATED, Json(r)).into_response())
}
