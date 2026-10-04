// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! HTTP surface, mounted by the gateway at `/balancers`.
//!
//! `GET|POST /balancers`, `GET|PUT|DELETE /balancers/{name-or-id}`,
//! the same for `/listeners` and `/pools`, members as a sub-resource
//! `GET|POST /pools/{id}/members`, `GET|PUT|DELETE /pools/{id}/members/{id}`,
//! and a flat `GET /members`. Handlers only validate the caller's scope and
//! hand off to `Client`; they never write `status`.
//!
//! The `Caller` comes from request extensions (set by the gateway).
//! Without one, the request is rejected.

use crate::client::Client;
use crate::types::*;
use axum::{
    extract::{Extension, Path, Query, Request, State},
    http::{header, HeaderMap, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
    Json,
};
use pistack_types::{kind, ApiError, ApiResult, Caller, Id, Kind, ListQuery, Meta, Page};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

const DEFAULT_LIMIT: usize = 100;
const MAX_LIMIT: usize = 1000;

pub fn router(client: Client) -> axum::Router {
    axum::Router::new()
        .route("/", get(info))
        .route("/balancers", get(list_balancers).post(create_balancer))
        .route(
            "/balancers/{id}",
            get(get_balancer)
                .put(update_balancer)
                .delete(delete_balancer),
        )
        .route("/listeners", get(list_listeners).post(create_listener))
        .route(
            "/listeners/{id}",
            get(get_listener)
                .put(update_listener)
                .delete(delete_listener),
        )
        .route("/pools", get(list_pools).post(create_pool))
        .route(
            "/pools/{id}",
            get(get_pool).put(update_pool).delete(delete_pool),
        )
        .route(
            "/pools/{id}/members",
            get(list_pool_members).post(create_member),
        )
        .route(
            "/pools/{id}/members/{member}",
            get(get_member).put(update_member).delete(delete_member),
        )
        .route("/members", get(list_members))
        .layer(middleware::from_fn(require_caller))
        .with_state(client)
}

/// The gateway attaches a `Caller`. A request without one is refused.
async fn require_caller(req: Request, next: Next) -> Response {
    if req.extensions().get::<Caller>().is_none() {
        return ApiError::unauthorized("no credentials").into_response();
    }
    next.run(req).await
}

// ---- request / response shapes ---------------------------------------------

#[derive(Deserialize)]
pub struct Create<S> {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    #[serde(default)]
    pub annotations: BTreeMap<String, String>,
    /// Admins may create in any project (or none); others inherit scope.
    #[serde(default)]
    pub project: Option<Id>,
    pub spec: S,
}

#[derive(Deserialize)]
pub struct Update<S> {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub labels: Option<BTreeMap<String, String>>,
    #[serde(default)]
    pub annotations: Option<BTreeMap<String, String>>,
    pub spec: S,
}

/// Member create under `/pools/{id}/members`: the pool comes from the path.
#[derive(Deserialize)]
pub struct MemberBody {
    pub address: std::net::IpAddr,
    pub port: u16,
    #[serde(default = "one")]
    pub weight: u16,
    #[serde(default)]
    pub backup: bool,
    #[serde(default = "yes")]
    pub admin_up: bool,
}

fn one() -> u16 {
    1
}
fn yes() -> bool {
    true
}

#[derive(Deserialize, Default)]
pub struct ScopeQuery {
    /// Admin-only: act in this project instead of the token's.
    #[serde(default)]
    pub project: Option<Id>,
    /// Admin-only: list every project.
    #[serde(default)]
    pub all: Option<bool>,
    /// Listeners and pools: restrict to one balancer (name or id).
    #[serde(default)]
    pub balancer: Option<String>,
    /// `/members`: restrict to one pool (name or id).
    #[serde(default)]
    pub pool: Option<String>,
}

#[derive(Serialize)]
struct Info {
    module: &'static str,
    providers: Vec<&'static str>,
    resources: &'static [&'static str],
}

async fn info(State(c): State<Client>) -> Json<Info> {
    Json(Info {
        module: "relay",
        providers: c.providers().names(),
        resources: KINDS,
    })
}

// ---- scope helpers --------------------------------------------------------

fn scope(caller: &Caller, q: &ScopeQuery) -> ApiResult<Option<Id>> {
    if caller.is_admin() {
        if q.all == Some(true) {
            return Ok(None);
        }
        return Ok(q.project.clone().or_else(|| caller.project.clone()));
    }
    if q.project.is_some() && q.project != caller.project {
        return Err(ApiError::forbidden("project scope does not match token"));
    }
    caller
        .project
        .clone()
        .map(Some)
        .ok_or_else(|| ApiError::forbidden("token has no project scope"))
}

fn create_project(caller: &Caller, body: &Option<Id>) -> ApiResult<Option<Id>> {
    match (caller.is_admin(), body, &caller.project) {
        (true, Some(p), _) => Ok(Some(p.clone())),
        (true, None, p) => Ok(p.clone()),
        (false, Some(p), Some(mine)) if p == mine => Ok(Some(p.clone())),
        (false, None, Some(mine)) => Ok(Some(mine.clone())),
        (false, _, _) => Err(ApiError::forbidden(
            "cannot create outside the token's project",
        )),
    }
}

fn visible<K: Kind>(caller: &Caller, r: K) -> ApiResult<K> {
    let p = &r.meta().project;
    if caller.is_admin() || p.is_none() || *p == caller.project {
        Ok(r)
    } else {
        Err(ApiError::not_found(format!(
            "{} {} not found",
            K::KIND,
            r.meta().id
        )))
    }
}

fn if_match(headers: &HeaderMap) -> ApiResult<Option<u64>> {
    match headers.get(header::IF_MATCH) {
        None => Ok(None),
        Some(v) => {
            let s = v
                .to_str()
                .map_err(|_| ApiError::bad_request("If-Match is not ascii"))?;
            let s = s.trim().trim_matches('"');
            s.parse::<u64>()
                .map(Some)
                .map_err(|_| ApiError::bad_request("If-Match must be a revision number"))
        }
    }
}

fn check_name(name: &str) -> ApiResult<&str> {
    let n = name.trim();
    if n.is_empty() || n.len() > 128 || n.contains('/') {
        return Err(ApiError::bad_request(
            "name must be 1-128 characters without '/'",
        ));
    }
    Ok(n)
}

fn new_meta<S>(k: &'static str, c: &Create<S>, project: Option<Id>) -> ApiResult<Meta> {
    let mut m = Meta::new(k, check_name(&c.name)?, project);
    m.description = c.description.clone();
    m.labels = c.labels.clone();
    m.annotations = c.annotations.clone();
    Ok(m)
}

fn apply_meta<S>(m: &mut Meta, u: &Update<S>) {
    if let Some(n) = &u.name {
        m.name = n.trim().to_owned();
    }
    if let Some(d) = &u.description {
        m.description = d.clone();
    }
    if let Some(l) = &u.labels {
        m.labels = l.clone();
    }
    if let Some(a) = &u.annotations {
        m.annotations = a.clone();
    }
}

fn page<K: Kind>(mut items: Vec<K>, q: &ListQuery) -> Page<K> {
    let sel = q.label_selector();
    items.retain(|k| {
        let m = k.meta();
        q.name.as_deref().is_none_or(|n| m.name == n)
            && sel.iter().all(|(lk, lv)| m.labels.get(lk) == Some(lv))
    });
    items.sort_by(|a, b| a.meta().id.cmp(&b.meta().id));
    let total = items.len();
    if let Some(after) = &q.after {
        items.retain(|k| k.meta().id.to_string().as_str() > after.as_str());
    }
    let limit = q.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let next = if items.len() > limit {
        items.get(limit - 1).map(|k| k.meta().id.to_string())
    } else {
        None
    };
    items.truncate(limit);
    Page { items, next, total }
}

fn created<T: Serialize>(t: T) -> Response {
    (StatusCode::CREATED, Json(t)).into_response()
}

// ---- one macro per kind for the uniform CRUD ------------------------------

macro_rules! crud {
    (
        $ty:ty, $spec:ty, $kind:expr, $col:ident, $filter:expr,
        $list:ident, $create:ident, $get:ident, $update:ident, $delete:ident,
        $c_create:ident, $c_update:ident, $c_delete:ident $(, $await:tt)?
    ) => {
        async fn $list(
            State(c): State<Client>,
            Extension(caller): Extension<Caller>,
            Query(sq): Query<ScopeQuery>,
            Query(q): Query<ListQuery>,
        ) -> ApiResult<Json<Page<$ty>>> {
            let project = scope(&caller, &sq)?;
            let mut items = c.$col().list(project.as_ref())?;
            let filter: fn(&Client, Option<&Id>, &ScopeQuery, &mut Vec<$ty>) -> ApiResult<()> = $filter;
            filter(&c, project.as_ref(), &sq, &mut items)?;
            Ok(Json(page(items, &q)))
        }

        async fn $create(
            State(c): State<Client>,
            Extension(caller): Extension<Caller>,
            Json(body): Json<Create<$spec>>,
        ) -> ApiResult<Response> {
            let project = create_project(&caller, &body.project)?;
            let meta = new_meta($kind, &body, project)?;
            Ok(created(c.$c_create(meta, body.spec)?))
        }

        async fn $get(
            State(c): State<Client>,
            Extension(caller): Extension<Caller>,
            Query(sq): Query<ScopeQuery>,
            Path(id): Path<String>,
        ) -> ApiResult<Json<$ty>> {
            let project = scope(&caller, &sq)?;
            Ok(Json(visible(
                &caller,
                c.$col().resolve(project.as_ref(), &id)?,
            )?))
        }

        async fn $update(
            State(c): State<Client>,
            Extension(caller): Extension<Caller>,
            Query(sq): Query<ScopeQuery>,
            Path(id): Path<String>,
            headers: HeaderMap,
            Json(body): Json<Update<$spec>>,
        ) -> ApiResult<Json<$ty>> {
            let project = scope(&caller, &sq)?;
            let cur = visible(&caller, c.$col().resolve(project.as_ref(), &id)?)?;
            let rev = if_match(&headers)?;
            Ok(Json(c.$c_update(&cur.meta.id, rev, |r| {
                apply_meta(&mut r.meta, &body);
                r.spec = body.spec.clone();
                Ok(())
            })?))
        }

        async fn $delete(
            State(c): State<Client>,
            Extension(caller): Extension<Caller>,
            Query(sq): Query<ScopeQuery>,
            Path(id): Path<String>,
        ) -> ApiResult<StatusCode> {
            let project = scope(&caller, &sq)?;
            let cur = visible(&caller, c.$col().resolve(project.as_ref(), &id)?)?;
            c.$c_delete(&cur.meta.id)$(.$await)??;
            Ok(StatusCode::NO_CONTENT)
        }
    };
}

fn by_balancer<K: Kind>(
    c: &Client,
    project: Option<&Id>,
    sq: &ScopeQuery,
    items: &mut Vec<K>,
    balancer_of: fn(&K) -> &Id,
) -> ApiResult<()> {
    if let Some(b) = &sq.balancer {
        let lb = c.balancers().resolve(project, b)?;
        items.retain(|k| balancer_of(k) == &lb.meta.id);
    }
    Ok(())
}

crud!(
    Balancer,
    BalancerSpec,
    kind::BALANCER,
    balancers,
    |_, _, _, _| Ok(()),
    list_balancers,
    create_balancer,
    get_balancer,
    update_balancer,
    delete_balancer,
    create_balancer,
    update_balancer,
    delete_balancer,
    await
);
crud!(
    Listener,
    ListenerSpec,
    kinds::LISTENER,
    listeners,
    |c, p, sq, items| by_balancer(c, p, sq, items, |l: &Listener| &l.spec.balancer),
    list_listeners,
    create_listener,
    get_listener,
    update_listener,
    delete_listener,
    create_listener,
    update_listener,
    delete_listener,
    await
);
crud!(
    Pool,
    PoolSpec,
    kinds::POOL,
    pools,
    |c, p, sq, items| by_balancer(c, p, sq, items, |x: &Pool| &x.spec.balancer),
    list_pools,
    create_pool,
    get_pool,
    update_pool,
    delete_pool,
    create_pool,
    update_pool,
    delete_pool
);

// ---- members ---------------------------------------------------------------

async fn list_members(
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

async fn list_pool_members(
    State(c): State<Client>,
    Extension(caller): Extension<Caller>,
    Query(sq): Query<ScopeQuery>,
    Query(q): Query<ListQuery>,
    Path(id): Path<String>,
) -> ApiResult<Json<Page<Member>>> {
    let pool = pool_in_scope(&c, &caller, &sq, &id)?;
    Ok(Json(page(c.members_of(&pool.meta.id)?, &q)))
}

async fn create_member(
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

async fn get_member(
    State(c): State<Client>,
    Extension(caller): Extension<Caller>,
    Query(sq): Query<ScopeQuery>,
    Path((id, member)): Path<(String, String)>,
) -> ApiResult<Json<Member>> {
    let pool = pool_in_scope(&c, &caller, &sq, &id)?;
    Ok(Json(member_of_pool(&c, &pool, &member)?))
}

async fn update_member(
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

async fn delete_member(
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
