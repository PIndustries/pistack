// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! HTTP surface, mounted by the gateway at `/network`.
//!
//! Plain nouns, one shape for every kind:
//! `GET|POST /{nouns}`, `GET|PUT|DELETE /{nouns}/{name-or-id}`, plus the
//! few verbs that are not a spec edit (router interfaces, security group
//! rules, floating IP association). Handlers only validate the caller's
//! scope and hand off to `Client`; they never write `status`.
//!
//! The `Caller` comes from request extensions (set by the gateway).
//! Without one, the request is rejected.

use crate::client::Client;
use crate::types::{self, *};
use axum::{
    extract::{Extension, Path, Query, Request, State},
    http::{header, HeaderMap, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post, put},
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
        .route("/networks", get(list_networks).post(create_network))
        .route(
            "/networks/{id}",
            get(get_network).put(update_network).delete(delete_network),
        )
        .route("/subnets", get(list_subnets).post(create_subnet))
        .route(
            "/subnets/{id}",
            get(get_subnet).put(update_subnet).delete(delete_subnet),
        )
        .route("/ports", get(list_ports).post(create_port))
        .route(
            "/ports/{id}",
            get(get_port).put(update_port).delete(delete_port),
        )
        .route("/routers", get(list_routers).post(create_router))
        .route(
            "/routers/{id}",
            get(get_router).put(update_router).delete(delete_router),
        )
        .route("/routers/{id}/interfaces", put(set_router_interfaces))
        .route("/security-groups", get(list_sgs).post(create_sg))
        .route(
            "/security-groups/{id}",
            get(get_sg).put(update_sg).delete(delete_sg),
        )
        .route("/security-groups/{id}/rules", post(add_rule))
        .route(
            "/security-groups/{id}/rules/{rule}",
            axum::routing::delete(remove_rule),
        )
        .route("/floating-ips", get(list_fips).post(create_fip))
        .route(
            "/floating-ips/{id}",
            get(get_fip).put(update_fip).delete(delete_fip),
        )
        .route("/floating-ips/{id}/associate", post(associate_fip))
        .route("/floating-ips/{id}/disassociate", post(disassociate_fip))
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

#[derive(Deserialize, Default)]
pub struct ScopeQuery {
    /// Admin-only: act in this project instead of the token's.
    #[serde(default)]
    pub project: Option<Id>,
    /// Admin-only: list every project.
    #[serde(default)]
    pub all: Option<bool>,
}

#[derive(Deserialize)]
pub struct Interfaces {
    pub subnets: Vec<Id>,
}

#[derive(Deserialize)]
pub struct Associate {
    /// Existing port. Exactly one of `port` or `machine` is set.
    #[serde(default)]
    pub port: Option<Id>,
    /// Machine whose current IPv4 becomes the fixed address.
    #[serde(default)]
    pub machine: Option<Id>,
}

#[derive(Serialize)]
struct Info {
    module: &'static str,
    backend: &'static str,
    resources: &'static [&'static str],
}

async fn info(State(c): State<Client>) -> Json<Info> {
    Json(Info {
        module: "fabric",
        backend: c.backend().name(),
        resources: KINDS,
    })
}

// ---- scope helpers --------------------------------------------------------

/// Resolve which project a list/get/create acts on.
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

/// Project for a create: body wins for admins, token wins otherwise.
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

/// The caller may see a resource in their project, a cloud-wide one, or
/// anything if admin. Others get not_found, not forbidden.
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

fn new_meta<S>(k: &'static str, c: &Create<S>, project: Option<Id>) -> ApiResult<Meta> {
    if c.name.trim().is_empty() || c.name.len() > 128 || c.name.contains('/') {
        return Err(ApiError::bad_request(
            "name must be 1-128 characters without '/'",
        ));
    }
    let mut m = Meta::new(k, c.name.trim(), project);
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

/// Filter by name/labels, then page by id (ids are time-ordered).
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
        $ty:ty, $spec:ty, $kind:expr, $col:ident,
        $list:ident, $create:ident, $get:ident, $update:ident, $delete:ident,
        $c_create:ident, $c_update:ident, $c_delete:ident
    ) => {
        async fn $list(
            State(c): State<Client>,
            Extension(caller): Extension<Caller>,
            Query(sq): Query<ScopeQuery>,
            Query(q): Query<ListQuery>,
        ) -> ApiResult<Json<Page<$ty>>> {
            let project = scope(&caller, &sq)?;
            let items = c.$col().list(project.as_ref())?;
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
            c.$c_delete(&cur.meta.id)?;
            Ok(StatusCode::NO_CONTENT)
        }
    };
}

crud!(
    Network,
    NetworkSpec,
    kind::NETWORK,
    networks,
    list_networks,
    create_network,
    get_network,
    update_network,
    delete_network,
    create_network,
    update_network,
    delete_network
);
crud!(
    Subnet,
    SubnetSpec,
    kind::SUBNET,
    subnets,
    list_subnets,
    create_subnet,
    get_subnet,
    update_subnet,
    delete_subnet,
    create_subnet,
    update_subnet,
    delete_subnet
);
crud!(
    Port,
    PortSpec,
    kind::PORT,
    ports,
    list_ports,
    create_port,
    get_port,
    update_port,
    delete_port,
    create_port,
    update_port,
    delete_port
);
crud!(
    types::Router,
    RouterSpec,
    kind::ROUTER,
    routers,
    list_routers,
    create_router,
    get_router,
    update_router,
    delete_router,
    create_router,
    update_router,
    delete_router
);
crud!(
    SecurityGroup,
    SecurityGroupSpec,
    kind::SECGROUP,
    security_groups,
    list_sgs,
    create_sg,
    get_sg,
    update_sg,
    delete_sg,
    create_security_group,
    update_security_group,
    delete_security_group
);
crud!(
    FloatingIp,
    FloatingIpSpec,
    kind::FLOATING_IP,
    floating_ips,
    list_fips,
    create_fip,
    get_fip,
    update_fip,
    delete_fip,
    create_floating_ip,
    update_floating_ip,
    delete_floating_ip
);

// ---- verbs ----------------------------------------------------------------

async fn set_router_interfaces(
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

async fn add_rule(
    State(c): State<Client>,
    Extension(caller): Extension<Caller>,
    Query(sq): Query<ScopeQuery>,
    Path(id): Path<String>,
    Json(rule): Json<Rule>,
) -> ApiResult<Response> {
    let project = scope(&caller, &sq)?;
    let cur = visible(&caller, c.security_groups().resolve(project.as_ref(), &id)?)?;
    Ok(created(c.add_rule(&cur.meta.id, rule)?))
}

async fn remove_rule(
    State(c): State<Client>,
    Extension(caller): Extension<Caller>,
    Query(sq): Query<ScopeQuery>,
    Path((id, rule)): Path<(String, uuid::Uuid)>,
) -> ApiResult<Json<SecurityGroup>> {
    let project = scope(&caller, &sq)?;
    let cur = visible(&caller, c.security_groups().resolve(project.as_ref(), &id)?)?;
    Ok(Json(c.remove_rule(&cur.meta.id, rule)?))
}

async fn associate_fip(
    State(c): State<Client>,
    Extension(caller): Extension<Caller>,
    Query(sq): Query<ScopeQuery>,
    Path(id): Path<String>,
    Json(body): Json<Associate>,
) -> ApiResult<Json<FloatingIp>> {
    let project = scope(&caller, &sq)?;
    let cur = visible(&caller, c.floating_ips().resolve(project.as_ref(), &id)?)?;
    let fip = match (body.port, body.machine) {
        (Some(port), None) => c.associate_floating_ip(&cur.meta.id, Some(port))?,
        (None, Some(machine)) => c.associate_to_machine(&cur.meta.id, &machine)?,
        _ => {
            return Err(ApiError::bad_request(
                "associate needs exactly one of port or machine",
            ))
        }
    };
    Ok(Json(fip))
}

async fn disassociate_fip(
    State(c): State<Client>,
    Extension(caller): Extension<Caller>,
    Query(sq): Query<ScopeQuery>,
    Path(id): Path<String>,
) -> ApiResult<Json<FloatingIp>> {
    let project = scope(&caller, &sq)?;
    let cur = visible(&caller, c.floating_ips().resolve(project.as_ref(), &id)?)?;
    Ok(Json(c.associate_floating_ip(&cur.meta.id, None)?))
}
