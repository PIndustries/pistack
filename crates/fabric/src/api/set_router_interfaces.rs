// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use crate::types::{self};
use axum::{
    extract::{Extension, Path, Query, State},
    http::HeaderMap,
    Json,
};
use pistack_types::{ApiResult, Caller};

use super::if_match::if_match;
use super::interfaces::Interfaces;
use super::scope::scope;
use super::scope_query::ScopeQuery;
use super::visible::visible;

pub(crate) async fn set_router_interfaces(
    State(c): State<Client>,
    Extension(caller): Extension<Caller>,
    Query(sq): Query<ScopeQuery>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<Interfaces>,
) -> ApiResult<Json<types::Router>> {
    let project = scope(&caller, &sq)?;
    let cur = visible(&caller, c.routers().resolve(project.as_ref(), &id)?)?;
    Ok(Json(c.set_router_interfaces(
        &cur.meta.id,
        if_match(&headers)?,
        &body.subnets,
    )?))
}
