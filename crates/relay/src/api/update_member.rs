// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `PUT /pools/{id}/members/{member}`: replace the member spec.
use super::body::{MemberBody, ScopeQuery};
use super::if_match::if_match;
use super::member_scope::{member_of_pool, pool_in_scope};
use crate::client::Client;
use crate::types::Member;
use axum::extract::{Extension, Path, Query, State};
use axum::http::HeaderMap;
use axum::Json;
use pistack_types::{ApiResult, Caller};

pub(super) async fn update_member(
    State(c): State<Client>,
    Extension(caller): Extension<Caller>,
    Query(sq): Query<ScopeQuery>,
    Path((id, member)): Path<(String, String)>,
    headers: HeaderMap,
    Json(body): Json<MemberBody>,
) -> ApiResult<Json<Member>> {
    let pool = pool_in_scope(&c, &caller, &sq, &id)?;
    let cur = member_of_pool(&c, &pool, &member)?;
    let rev = if_match(&headers)?;
    Ok(Json(c.update_member(&cur.meta.id, rev, |m| {
        m.spec.address = body.address;
        m.spec.port = body.port;
        m.spec.weight = body.weight;
        m.spec.backup = body.backup;
        m.spec.admin_up = body.admin_up;
        Ok(())
    })?))
}
