// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Members: sub-resources of a pool, so the pool is resolved first and the
//! member must belong to it.

use super::body::{MemberBody, ScopeQuery};
use super::created::created;
use super::if_match::if_match;
use super::name::check_name;
use super::page::page;
use super::scope::scope;
use super::visible::visible;
use crate::client::Client;
use crate::types::{kinds, Member, MemberSpec, Pool};
use axum::extract::{Extension, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use axum::Json;
use pistack_types::{ApiError, ApiResult, Caller, ListQuery, Meta, Page};

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

fn pool_in_scope(c: &Client, caller: &Caller, sq: &ScopeQuery, id: &str) -> ApiResult<Pool> {
    let project = scope(caller, sq)?;
    visible(caller, c.pools().resolve(project.as_ref(), id)?)
}

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

fn member_of_pool(c: &Client, pool: &Pool, member: &str) -> ApiResult<Member> {
    let m = c.members().resolve(pool.meta.project.as_ref(), member)?;
    if m.spec.pool != pool.meta.id {
        return Err(ApiError::not_found(format!(
            "member {member} not found in pool {}",
            pool.meta.id
        )));
    }
    Ok(m)
}

pub(super) async fn get_member(
    State(c): State<Client>,
    Extension(caller): Extension<Caller>,
    Query(sq): Query<ScopeQuery>,
    Path((id, member)): Path<(String, String)>,
) -> ApiResult<Json<Member>> {
    let pool = pool_in_scope(&c, &caller, &sq, &id)?;
    Ok(Json(member_of_pool(&c, &pool, &member)?))
}

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
