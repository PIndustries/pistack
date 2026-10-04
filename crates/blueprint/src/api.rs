// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! HTTP surface, mounted by the gateway at `/blueprint`.
//!
//! `GET|POST /stacks`, `GET|DELETE /stacks/{id}`, `POST /stacks/{id}/apply`.
//! Apply re-validates the stored document, creates network, subnet and
//! machine rows from it, and leaves `status.phase` at `accepted`. Unknown
//! kinds are skipped.
//!
//! The `Caller` comes from request extensions (set by the gateway). Without
//! one, the request is rejected.

use crate::client::Client;
use crate::types::{Stack, StackSpec};
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
        resources: &["stacks"],
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

fn visible(caller: &Caller, stack: Stack) -> ApiResult<Stack> {
    let p = &stack.meta.project;
    if caller.is_admin() || p.is_none() || *p == caller.project {
        Ok(stack)
    } else {
        Err(ApiError::not_found(format!(
            "{} {} not found",
            kind::STACK,
            stack.meta.id
        )))
    }
}

fn new_meta(body: &CreateStack, project: Option<Id>) -> ApiResult<Meta> {
    let name = body.name.trim();
    if name.is_empty() || name.len() > 128 || name.contains('/') {
        return Err(ApiError::bad_request(
            "name must be 1-128 characters without '/'",
        ));
    }
    let mut meta = Meta::new(kind::STACK, name, project);
    meta.description = body.description.clone();
    meta.labels = body.labels.clone();
    meta.annotations = body.annotations.clone();
    Ok(meta)
}

fn page(mut items: Vec<Stack>, q: &ListQuery) -> Page<Stack> {
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

fn created(stack: Stack) -> Response {
    (StatusCode::CREATED, Json(stack)).into_response()
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
    let meta = new_meta(&body, project)?;
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
