// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! HTTP surface, mounted by the gateway at `/dns`.
//!
//! `GET|POST /zones`, `GET|PUT|DELETE /zones/{name-or-id}`,
//! `GET /zones/{id}/export` (master-file text),
//! `GET|POST /recordsets`, `GET|PUT|DELETE /recordsets/{name-or-id}`.
//! Handlers only validate the caller's scope and hand off to `Client`;
//! they never write `status`.
//!
//! The `Caller` comes from request extensions (set by the gateway once
//! `gate` is wired). Without one, requests run as a development admin.

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
        .route("/zones", get(list_zones).post(create_zone))
        .route(
            "/zones/{id}",
            get(get_zone).put(update_zone).delete(delete_zone),
        )
        .route("/zones/{id}/export", get(export_zone_text))
        .route("/recordsets", get(list_recordsets).post(create_recordset))
        .route(
            "/recordsets/{id}",
            get(get_recordset)
                .put(update_recordset)
                .delete(delete_recordset),
        )
        .layer(middleware::from_fn(default_caller))
        .with_state(client)
}

/// Attach a development admin `Caller` when the gateway did not.
async fn default_caller(mut req: Request, next: Next) -> Response {
    if req.extensions().get::<Caller>().is_none() {
        req.extensions_mut().insert(dev_admin());
    }
    next.run(req).await
}

pub fn dev_admin() -> Caller {
    Caller {
        principal: Id::new(kind::PRINCIPAL),
        principal_name: "dev-admin".into(),
        project: None,
        roles: vec!["admin".into()],
        system: true,
    }
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
    /// Record sets only: restrict to one zone (name or id).
    #[serde(default)]
    pub zone: Option<String>,
}

#[derive(Serialize)]
struct Info {
    module: &'static str,
    resources: &'static [&'static str],
}

async fn info() -> Json<Info> {
    Json(Info {
        module: "beacon",
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

// ---- zones ------------------------------------------------------------------

async fn list_zones(
    State(c): State<Client>,
    Extension(caller): Extension<Caller>,
    Query(sq): Query<ScopeQuery>,
    Query(q): Query<ListQuery>,
) -> ApiResult<Json<Page<Zone>>> {
    let project = scope(&caller, &sq)?;
    Ok(Json(page(c.zones().list(project.as_ref())?, &q)))
}

async fn create_zone(
    State(c): State<Client>,
    Extension(caller): Extension<Caller>,
    Json(body): Json<Create<ZoneSpec>>,
) -> ApiResult<Response> {
    let project = create_project(&caller, &body.project)?;
    let meta = new_meta(kind::ZONE, &body, project)?;
    Ok(created(c.create_zone(meta, body.spec)?))
}

/// Zones resolve by resource name, id, or apex (`example.cloud.`).
fn resolve_zone(c: &Client, project: Option<&Id>, id: &str) -> ApiResult<Zone> {
    match c.zones().resolve(project, id) {
        Ok(z) => Ok(z),
        Err(e) if id.contains('.') => c.zone_by_apex(id)?.ok_or_else(|| ApiError::from(e)),
        Err(e) => Err(e.into()),
    }
}

async fn get_zone(
    State(c): State<Client>,
    Extension(caller): Extension<Caller>,
    Query(sq): Query<ScopeQuery>,
    Path(id): Path<String>,
) -> ApiResult<Json<Zone>> {
    let project = scope(&caller, &sq)?;
    Ok(Json(visible(
        &caller,
        resolve_zone(&c, project.as_ref(), &id)?,
    )?))
}

async fn update_zone(
    State(c): State<Client>,
    Extension(caller): Extension<Caller>,
    Query(sq): Query<ScopeQuery>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<Update<ZoneSpec>>,
) -> ApiResult<Json<Zone>> {
    let project = scope(&caller, &sq)?;
    let cur = visible(&caller, resolve_zone(&c, project.as_ref(), &id)?)?;
    let rev = if_match(&headers)?;
    Ok(Json(c.update_zone(&cur.meta.id, rev, |z| {
        apply_meta(&mut z.meta, &body);
        z.spec = body.spec.clone();
        Ok(())
    })?))
}

async fn delete_zone(
    State(c): State<Client>,
    Extension(caller): Extension<Caller>,
    Query(sq): Query<ScopeQuery>,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    let project = scope(&caller, &sq)?;
    let cur = visible(&caller, resolve_zone(&c, project.as_ref(), &id)?)?;
    c.delete_zone(&cur.meta.id)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn export_zone_text(
    State(c): State<Client>,
    Extension(caller): Extension<Caller>,
    Query(sq): Query<ScopeQuery>,
    Path(id): Path<String>,
) -> ApiResult<Response> {
    let project = scope(&caller, &sq)?;
    let cur = visible(&caller, resolve_zone(&c, project.as_ref(), &id)?)?;
    let text = c.export(&cur.meta.id)?;
    Ok(([(header::CONTENT_TYPE, "text/dns; charset=utf-8")], text).into_response())
}

// ---- record sets ------------------------------------------------------------

async fn list_recordsets(
    State(c): State<Client>,
    Extension(caller): Extension<Caller>,
    Query(sq): Query<ScopeQuery>,
    Query(q): Query<ListQuery>,
) -> ApiResult<Json<Page<RecordSet>>> {
    let project = scope(&caller, &sq)?;
    let mut items = c.recordsets().list(project.as_ref())?;
    if let Some(z) = &sq.zone {
        let zone = resolve_zone(&c, project.as_ref(), z)?;
        items.retain(|r| r.spec.zone == zone.meta.id);
    }
    Ok(Json(page(items, &q)))
}

async fn create_recordset(
    State(c): State<Client>,
    Extension(caller): Extension<Caller>,
    Json(body): Json<Create<RecordSetSpec>>,
) -> ApiResult<Response> {
    let project = create_project(&caller, &body.project)?;
    let meta = new_meta(kind::RECORD, &body, project)?;
    Ok(created(c.create_recordset(meta, body.spec)?))
}

async fn get_recordset(
    State(c): State<Client>,
    Extension(caller): Extension<Caller>,
    Query(sq): Query<ScopeQuery>,
    Path(id): Path<String>,
) -> ApiResult<Json<RecordSet>> {
    let project = scope(&caller, &sq)?;
    Ok(Json(visible(
        &caller,
        c.recordsets().resolve(project.as_ref(), &id)?,
    )?))
}

async fn update_recordset(
    State(c): State<Client>,
    Extension(caller): Extension<Caller>,
    Query(sq): Query<ScopeQuery>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<Update<RecordSetSpec>>,
) -> ApiResult<Json<RecordSet>> {
    let project = scope(&caller, &sq)?;
    let cur = visible(&caller, c.recordsets().resolve(project.as_ref(), &id)?)?;
    let rev = if_match(&headers)?;
    Ok(Json(c.update_recordset(&cur.meta.id, rev, |r| {
        apply_meta(&mut r.meta, &body);
        r.spec = body.spec.clone();
        Ok(())
    })?))
}

async fn delete_recordset(
    State(c): State<Client>,
    Extension(caller): Extension<Caller>,
    Query(sq): Query<ScopeQuery>,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    let project = scope(&caller, &sq)?;
    let cur = visible(&caller, c.recordsets().resolve(project.as_ref(), &id)?)?;
    c.delete_recordset(&cur.meta.id)?;
    Ok(StatusCode::NO_CONTENT)
}
