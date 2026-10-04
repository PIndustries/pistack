// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! HTTP surface, mounted by the gateway at `/gauge`.
//!
//! `POST /samples` `{metric, value}` records a point and re-evaluates alarms
//! on that metric. `GET /samples?metric=` lists the ring.
//! `GET|POST /alarms`, `GET|DELETE /alarms/{id}`.
//! `GET /metrics` is Prometheus text (latest sample per metric).
//!
//! The `Caller` comes from request extensions (set by the gateway). Without
//! one, the request is rejected.

use crate::client::Client;
use crate::types::{Alarm, AlarmSpec, Sample};
use axum::{
    extract::{Extension, Path, Query, Request, State},
    http::{header, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
    Json,
};
use pistack_types::{kind, ApiError, ApiResult, Caller, Id, Kind, ListQuery, Meta, Page};
use serde::Deserialize;
use std::collections::BTreeMap;

const DEFAULT_LIMIT: usize = 100;
const MAX_LIMIT: usize = 1000;

pub fn router(client: Client) -> axum::Router {
    axum::Router::new()
        .route("/samples", get(list_samples).post(record_sample))
        .route("/alarms", get(list_alarms).post(create_alarm))
        .route("/alarms/{id}", get(get_alarm).delete(delete_alarm))
        .route("/metrics", get(metrics))
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

#[derive(Deserialize)]
struct Create<S> {
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    labels: BTreeMap<String, String>,
    #[serde(default)]
    annotations: BTreeMap<String, String>,
    #[serde(default)]
    project: Option<Id>,
    spec: S,
}

#[derive(Deserialize, Default)]
struct ScopeQuery {
    #[serde(default)]
    project: Option<Id>,
    #[serde(default)]
    all: Option<bool>,
}

#[derive(Deserialize)]
struct SampleIn {
    metric: String,
    value: f64,
}

#[derive(Deserialize, Default)]
struct SampleQuery {
    #[serde(default)]
    metric: Option<String>,
}

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

fn visible<K: Kind>(caller: &Caller, resource: K) -> ApiResult<K> {
    let project = &resource.meta().project;
    if caller.is_admin() || project.is_none() || *project == caller.project {
        Ok(resource)
    } else {
        Err(ApiError::not_found(format!(
            "{} {} not found",
            K::KIND,
            resource.meta().id
        )))
    }
}

fn new_meta<S>(kind_: &'static str, body: &Create<S>, project: Option<Id>) -> ApiResult<Meta> {
    let name = body.name.trim();
    if name.is_empty() || name.len() > 128 || name.contains('/') {
        return Err(ApiError::bad_request(
            "name must be 1-128 characters without '/'",
        ));
    }
    let mut meta = Meta::new(kind_, name, project);
    meta.description = body.description.clone();
    meta.labels = body.labels.clone();
    meta.annotations = body.annotations.clone();
    Ok(meta)
}

fn page<K: Kind>(mut items: Vec<K>, q: &ListQuery) -> Page<K> {
    let sel = q.label_selector();
    items.retain(|k| {
        let meta = k.meta();
        q.name.as_deref().is_none_or(|n| meta.name == n)
            && sel.iter().all(|(lk, lv)| meta.labels.get(lk) == Some(lv))
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

fn created<T: serde::Serialize>(body: T) -> Response {
    (StatusCode::CREATED, Json(body)).into_response()
}

async fn record_sample(
    State(client): State<Client>,
    Json(body): Json<SampleIn>,
) -> ApiResult<Response> {
    Ok(created(client.record_sample(body.metric, body.value)?))
}

async fn list_samples(
    State(client): State<Client>,
    Query(q): Query<SampleQuery>,
) -> Json<Vec<Sample>> {
    let metric = q.metric.as_deref().filter(|m| !m.is_empty());
    Json(client.list_samples(metric))
}

async fn list_alarms(
    State(client): State<Client>,
    Extension(caller): Extension<Caller>,
    Query(sq): Query<ScopeQuery>,
    Query(q): Query<ListQuery>,
) -> ApiResult<Json<Page<Alarm>>> {
    let project = scope(&caller, &sq)?;
    Ok(Json(page(client.list_alarms(project.as_ref())?, &q)))
}

async fn create_alarm(
    State(client): State<Client>,
    Extension(caller): Extension<Caller>,
    Json(body): Json<Create<AlarmSpec>>,
) -> ApiResult<Response> {
    let project = create_project(&caller, &body.project)?;
    let meta = new_meta(kind::ALARM, &body, project)?;
    Ok(created(client.create_alarm(meta, body.spec)?))
}

async fn get_alarm(
    State(client): State<Client>,
    Extension(caller): Extension<Caller>,
    Query(sq): Query<ScopeQuery>,
    Path(id): Path<String>,
) -> ApiResult<Json<Alarm>> {
    let project = scope(&caller, &sq)?;
    Ok(Json(visible(
        &caller,
        client.get_alarm(project.as_ref(), &id)?,
    )?))
}

async fn delete_alarm(
    State(client): State<Client>,
    Extension(caller): Extension<Caller>,
    Query(sq): Query<ScopeQuery>,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    let project = scope(&caller, &sq)?;
    let alarm = visible(&caller, client.get_alarm(project.as_ref(), &id)?)?;
    client.delete_alarm(&alarm.meta.id)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn metrics(State(client): State<Client>) -> impl IntoResponse {
    (
        [(
            header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        client.metrics_text(),
    )
}
