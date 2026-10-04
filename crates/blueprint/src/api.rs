// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! HTTP surface, mounted by the gateway at `/blueprint`.
//!
//! `GET|POST /stacks`, `GET|DELETE /stacks/{id}`, `POST /stacks/{id}/apply`,
//! and the same routes for `/kits` and `/workflows`. Apply re-validates the
//! stored document and creates network, subnet, machine, router, pod,
//! listener, and record set rows from it. A kit is that same document. A
//! workflow applies named stacks in order and stops on the first error.
//! Unknown kinds are skipped. `status.phase` stays `accepted` on a stack
//! or kit.
//!
//! The `Caller` comes from request extensions (set by the gateway). Without
//! one, the request is rejected.

use crate::client::Client;
use crate::types::{
    Kit, KitSpec, Stack, StackSpec, Workflow, WorkflowSpec, KIT_KIND, WORKFLOW_KIND,
};
use axum::{
    extract::{Extension, Path, Query, Request, State},
    http::StatusCode,
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
    Json,
};
use pistack_types::{kind, ApiError, ApiResult, Caller, Id, Kind, ListQuery, Meta, Page};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub fn router(client: Client) -> axum::Router {
    axum::Router::new()
        .route("/", get(info))
        .route("/stacks", get(list_stacks).post(create_stack))
        .route("/stacks/{id}", get(get_stack).delete(delete_stack))
        .route("/stacks/{id}/apply", axum::routing::post(apply_stack))
        .route("/kits", get(list_kits).post(create_kit))
        .route("/kits/{id}", get(get_kit).delete(delete_kit))
        .route("/kits/{id}/apply", axum::routing::post(apply_kit))
        .route("/workflows", get(list_workflows).post(create_workflow))
        .route("/workflows/{id}", get(get_workflow).delete(delete_workflow))
        .route("/workflows/{id}/apply", axum::routing::post(apply_workflow))
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
pub struct CreateStack {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    #[serde(default)]
    pub annotations: BTreeMap<String, String>,
    #[serde(default)]
    pub project: Option<Id>,
    pub spec: StackSpec,
}

#[derive(Serialize)]
struct Info {
    module: &'static str,
    resources: &'static [&'static str],
}

async fn info() -> Json<Info> {
    Json(Info {
        module: "blueprint",
        resources: &["stacks", "kits", "workflows"],
    })
}

fn list_project(caller: &Caller) -> ApiResult<Option<Id>> {
    if caller.is_admin() {
        return Ok(caller.project.clone());
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

fn visible<K: Kind>(caller: &Caller, item: K) -> ApiResult<K> {
    let project = item.meta().project.clone();
    if caller.is_admin() || project.is_none() || project == caller.project {
        Ok(item)
    } else {
        Err(ApiError::not_found(format!(
            "{} {} not found",
            K::KIND,
            item.meta().id
        )))
    }
}

fn new_meta(
    kind_name: &'static str,
    name: &str,
    description: String,
    labels: BTreeMap<String, String>,
    annotations: BTreeMap<String, String>,
    project: Option<Id>,
) -> ApiResult<Meta> {
    let name = name.trim();
    if name.is_empty() || name.len() > 128 || name.contains('/') {
        return Err(ApiError::bad_request(
            "name must be 1-128 characters without '/'",
        ));
    }
    let mut meta = Meta::new(kind_name, name, project);
    meta.description = description;
    meta.labels = labels;
    meta.annotations = annotations;
    Ok(meta)
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
    if let Some(limit) = q.limit {
        items.truncate(limit);
    }
    Page {
        items,
        next: None,
        total,
    }
}

fn created<T: Serialize>(item: T) -> Response {
    (StatusCode::CREATED, Json(item)).into_response()
}

async fn list_stacks(
    State(client): State<Client>,
    Extension(caller): Extension<Caller>,
    Query(q): Query<ListQuery>,
) -> ApiResult<Json<Page<Stack>>> {
    let project = list_project(&caller)?;
    Ok(Json(page(client.list_stacks(project.as_ref())?, &q)))
}

async fn create_stack(
    State(client): State<Client>,
    Extension(caller): Extension<Caller>,
    Json(body): Json<CreateStack>,
) -> ApiResult<Response> {
    let project = create_project(&caller, &body.project)?;
    let meta = new_meta(
        kind::STACK,
        &body.name,
        body.description,
        body.labels,
        body.annotations,
        project,
    )?;
    Ok(created(client.create_stack(meta, body.spec)?))
}

async fn get_stack(
    State(client): State<Client>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<String>,
) -> ApiResult<Json<Stack>> {
    let project = list_project(&caller)?;
    Ok(Json(visible(
        &caller,
        client.resolve_stack(project.as_ref(), &id)?,
    )?))
}

async fn delete_stack(
    State(client): State<Client>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    let project = list_project(&caller)?;
    let cur = visible(&caller, client.resolve_stack(project.as_ref(), &id)?)?;
    client.delete_stack(&cur.meta.id)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn apply_stack(
    State(client): State<Client>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<String>,
) -> ApiResult<Json<Stack>> {
    let project = list_project(&caller)?;
    let cur = visible(&caller, client.resolve_stack(project.as_ref(), &id)?)?;
    Ok(Json(client.apply_stack(&cur.meta.id)?))
}

#[derive(Deserialize)]
pub struct CreateKit {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    #[serde(default)]
    pub annotations: BTreeMap<String, String>,
    #[serde(default)]
    pub project: Option<Id>,
    pub spec: KitSpec,
}

#[derive(Deserialize)]
pub struct CreateWorkflow {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    #[serde(default)]
    pub annotations: BTreeMap<String, String>,
    #[serde(default)]
    pub project: Option<Id>,
    pub spec: WorkflowSpec,
}

async fn list_kits(
    State(client): State<Client>,
    Extension(caller): Extension<Caller>,
    Query(q): Query<ListQuery>,
) -> ApiResult<Json<Page<Kit>>> {
    let project = list_project(&caller)?;
    Ok(Json(page(client.list_kits(project.as_ref())?, &q)))
}

async fn create_kit(
    State(client): State<Client>,
    Extension(caller): Extension<Caller>,
    Json(body): Json<CreateKit>,
) -> ApiResult<Response> {
    let project = create_project(&caller, &body.project)?;
    let meta = new_meta(
        KIT_KIND,
        &body.name,
        body.description,
        body.labels,
        body.annotations,
        project,
    )?;
    Ok(created(client.create_kit(meta, body.spec)?))
}

async fn get_kit(
    State(client): State<Client>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<String>,
) -> ApiResult<Json<Kit>> {
    let project = list_project(&caller)?;
    Ok(Json(visible(
        &caller,
        client.resolve_kit(project.as_ref(), &id)?,
    )?))
}

async fn delete_kit(
    State(client): State<Client>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    let project = list_project(&caller)?;
    let cur = visible(&caller, client.resolve_kit(project.as_ref(), &id)?)?;
    client.delete_kit(&cur.meta.id)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn apply_kit(
    State(client): State<Client>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<String>,
) -> ApiResult<Json<Kit>> {
    let project = list_project(&caller)?;
    let cur = visible(&caller, client.resolve_kit(project.as_ref(), &id)?)?;
    Ok(Json(client.apply_kit(&cur.meta.id)?))
}

async fn list_workflows(
    State(client): State<Client>,
    Extension(caller): Extension<Caller>,
    Query(q): Query<ListQuery>,
) -> ApiResult<Json<Page<Workflow>>> {
    let project = list_project(&caller)?;
    Ok(Json(page(client.list_workflows(project.as_ref())?, &q)))
}

async fn create_workflow(
    State(client): State<Client>,
    Extension(caller): Extension<Caller>,
    Json(body): Json<CreateWorkflow>,
) -> ApiResult<Response> {
    let project = create_project(&caller, &body.project)?;
    let meta = new_meta(
        WORKFLOW_KIND,
        &body.name,
        body.description,
        body.labels,
        body.annotations,
        project,
    )?;
    Ok(created(client.create_workflow(meta, body.spec)?))
}

async fn get_workflow(
    State(client): State<Client>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<String>,
) -> ApiResult<Json<Workflow>> {
    let project = list_project(&caller)?;
    Ok(Json(visible(
        &caller,
        client.resolve_workflow(project.as_ref(), &id)?,
    )?))
}

async fn delete_workflow(
    State(client): State<Client>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    let project = list_project(&caller)?;
    let cur = visible(&caller, client.resolve_workflow(project.as_ref(), &id)?)?;
    client.delete_workflow(&cur.meta.id)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn apply_workflow(
    State(client): State<Client>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<String>,
) -> ApiResult<Json<Workflow>> {
    let project = list_project(&caller)?;
    let cur = visible(&caller, client.resolve_workflow(project.as_ref(), &id)?)?;
    Ok(Json(client.apply_workflow(&cur.meta.id)?))
}
