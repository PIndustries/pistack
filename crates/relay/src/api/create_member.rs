// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `POST /pools/{id}/members`: add a member; the pool and address name it.
use super::body::{MemberBody, ScopeQuery};
use super::created::created;
use super::member_scope::pool_in_scope;
use super::name::check_name;
use crate::client::Client;
use crate::types::{kinds, MemberSpec};
use axum::extract::{Extension, Json, Path, Query, State};
use axum::response::Response;
use pistack_types::{ApiResult, Caller, Meta};

pub(super) async fn create_member(
    State(c): State<Client>,
    Extension(caller): Extension<Caller>,
    Query(sq): Query<ScopeQuery>,
    Path(id): Path<String>,
    Json(body): Json<MemberBody>,
) -> ApiResult<Response> {
    let pool = pool_in_scope(&c, &caller, &sq, &id)?;
    let name = format!("{}-{}-{}", pool.meta.name, body.address, body.port).replace(':', "-");
    let meta = Meta::new(kinds::MEMBER, check_name(&name)?, pool.meta.project.clone());
    let spec = MemberSpec {
        pool: pool.meta.id.clone(),
        address: body.address,
        port: body.port,
        weight: body.weight,
        backup: body.backup,
        admin_up: body.admin_up,
    };
    Ok(created(c.create_member(meta, spec)?))
}
