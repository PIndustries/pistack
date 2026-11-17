// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `GET /pools/{id}/members`: the members of one pool.
use super::body::ScopeQuery;
use super::member_scope::pool_in_scope;
use super::page::page;
use crate::client::Client;
use crate::types::Member;
use axum::extract::{Extension, Path, Query, State};
use axum::Json;
use pistack_types::{ApiResult, Caller, ListQuery, Page};

pub(super) async fn list_pool_members(
    State(c): State<Client>,
    Extension(caller): Extension<Caller>,
    Query(sq): Query<ScopeQuery>,
    Query(q): Query<ListQuery>,
    Path(id): Path<String>,
) -> ApiResult<Json<Page<Member>>> {
    let pool = pool_in_scope(&c, &caller, &sq, &id)?;
    Ok(Json(page(c.members_of(&pool.meta.id)?, &q)))
}
