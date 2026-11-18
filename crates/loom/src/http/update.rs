// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::*;
use axum::{
    extract::{Path, State},
    http::HeaderMap,
    Json,
};
use pistack_types::{ApiError, ApiResult};
use serde::{de::DeserializeOwned, Deserialize};
use std::collections::BTreeMap;

use crate::http::api::Api;
use crate::http::app_state::AppState;
use crate::http::body::Body;
use crate::http::col::col;
use crate::http::if_match::if_match;
use crate::http::load::load;
use crate::http::res::Res;
use crate::http::scope::Scope;

#[derive(Debug, Deserialize)]
#[serde(bound(deserialize = "S: DeserializeOwned"))]
pub struct Update<S> {
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub labels: Option<BTreeMap<String, String>>,
    #[serde(default)]
    pub annotations: Option<BTreeMap<String, String>>,
    #[serde(default)]
    pub spec: Option<S>,
}

pub(crate) async fn update<A: Api>(
    State(s): State<AppState>,
    scope: Scope,
    Path(id): Path<String>,
    headers: HeaderMap,
    Body(body): Body<Update<A>>,
) -> ApiResult<Json<Res<A>>> {
    let expected = if_match(&headers)?;
    let mut r = load::<A>(&s, &scope, &id)?;
    if delete_requested(&r) {
        return Err(ApiError::conflict("resource is being deleted"));
    }
    if let Some(d) = body.description {
        r.meta.description = d;
    }
    if let Some(l) = body.labels {
        r.meta.labels = l;
    }
    if let Some(a) = body.annotations {
        let keep = r.meta.annotations.get(DELETE_ANNOTATION).cloned();
        r.meta.annotations = a;
        r.meta.annotations.remove(DELETE_ANNOTATION);
        if let Some(v) = keep {
            r.meta.annotations.insert(DELETE_ANNOTATION.into(), v);
        }
    }
    if let Some(mut spec) = body.spec {
        spec.validate(Some(&r))?;
        r.spec = spec;
    }
    Ok(Json(col::<A>(&s).update(r, expected)?))
}
