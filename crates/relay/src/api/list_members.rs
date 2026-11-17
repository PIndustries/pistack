// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `GET /members`: every member, optionally narrowed to one pool.
use super::body::ScopeQuery;
use super::page::page;
use super::scope::scope;
use crate::client::Client;
use crate::types::Member;
use axum::extract::{Extension, Query, State};
use axum::Json;
use pistack_types::{ApiResult, Caller, ListQuery, Page};

pub(super) async fn list_members(
    State(c): State<Client>,
    Extension(caller): Extension<Caller>,
    Query(sq): Query<ScopeQuery>,
    Query(q): Query<ListQuery>,
) -> ApiResult<Json<Page<Member>>> {
    let project = scope(&caller, &sq)?;
    let mut items = c.members().list(project.as_ref())?;
    if let Some(p) = &sq.pool {
        let pool = c.pools().resolve(project.as_ref(), p)?;
        items.retain(|m| m.spec.pool == pool.meta.id);
    }
    Ok(Json(page(items, &q)))
}
