// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use crate::types::*;
use axum::{
    extract::{Extension, Path, Query, State},
    http::HeaderMap,
    Json,
};
use pistack_types::{ApiResult, Caller};

use super::apply_meta::apply_meta;
use super::guarded::Guarded;
use super::if_match::if_match;
use super::scope::scope;
use super::scope_query::ScopeQuery;
use super::update::Update;
use super::visible::visible;
use crate::types::VpcSpec;

pub(crate) async fn update_vpc(
    State(c): State<Client>,
    Extension(caller): Extension<Caller>,
    Query(sq): Query<ScopeQuery>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<Update<VpcSpec>>,
) -> ApiResult<Json<Vpc>> {
    let project = scope(&caller, &sq)?;
    let cur = visible(&caller, c.vpcs().resolve(project.as_ref(), &id)?)?;
    let rev = if_match(&headers)?;
    body.spec.guard(&caller, Some(&cur.spec))?;
    Ok(Json(c.update_vpc(&cur.meta.id, rev, |v| {
        apply_meta(&mut v.meta, &body);
        v.spec = body.spec.clone();
        Ok(())
    })?))
}
