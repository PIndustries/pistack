// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `DELETE /pools/{id}/members/{member}`.
use super::body::ScopeQuery;
use super::member_scope::{member_of_pool, pool_in_scope};
use crate::client::Client;
use axum::extract::{Extension, Path, Query, State};
use axum::http::StatusCode;
use pistack_types::{ApiResult, Caller};

pub(super) async fn delete_member(
    State(c): State<Client>,
    Extension(caller): Extension<Caller>,
    Query(sq): Query<ScopeQuery>,
    Path((id, member)): Path<(String, String)>,
) -> ApiResult<StatusCode> {
    let pool = pool_in_scope(&c, &caller, &sq, &id)?;
    let cur = member_of_pool(&c, &pool, &member)?;
    c.delete_member(&cur.meta.id)?;
    Ok(StatusCode::NO_CONTENT)
}
