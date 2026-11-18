// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::*;
use axum::{
    extract::{Extension, Path, Query, State},
    http::HeaderMap,
    Json,
};
use pistack_types::{ApiResult, Caller};

use super::api_state::ApiState;
use super::apply_meta::apply_meta;
use super::asked::asked;
use super::if_match::if_match;
use super::resolve_zone::resolve_zone;
use super::scope::scope;
use super::scope_query::ScopeQuery;
use super::update::Update;
use super::visible::visible;

pub(crate) async fn update_zone(
    State(st): State<ApiState>,
    Extension(caller): Extension<Caller>,
    Query(sq): Query<ScopeQuery>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<Update<ZoneSpec>>,
) -> ApiResult<Json<Zone>> {
    let project = scope(&caller, &sq)?;
    let cur = visible(&caller, resolve_zone(&st.client, project.as_ref(), &id)?)?;
    let rev = if_match(&headers)?;
    let z = st.client.update_zone(&cur.meta.id, rev, |z| {
        apply_meta(&mut z.meta, &body);
        z.spec = body.spec.clone();
        Ok(())
    })?;
    asked(&st, &z.spec.name);
    Ok(Json(z))
}
