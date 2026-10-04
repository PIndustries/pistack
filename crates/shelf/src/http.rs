// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! HTTP surface, mounted by the gateway at `/shelf`.
//!
//! `GET|POST /shares`, `GET|PUT|DELETE /shares/{id}`.
//! `PUT` replaces `spec` and bumps `revision`. `If-Match` is that revision.
//!
//! The gateway attaches a [`Caller`]. Without one the request is refused.
//! Lists are limited to the caller's project unless `caller.system`.

use axum::{
    extract::{Path, Query, Request, State},
    http::{header, HeaderMap, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    Json, Router,
};
use pistack_types::{ApiError, ApiResult, Caller, Kind, ListQuery, Page};
use serde::Deserialize;
use std::collections::BTreeMap;

use crate::realize;
use crate::types::{assign_name, Share, ShareSpec};

pub fn router(store: pistack_store::Store) -> Router {
    Router::new()
        .route(
            "/shares",
            axum::routing::get(list_shares).post(create_share),
        )
        .route(
            "/shares/{id}",
            axum::routing::get(get_share)
                .put(update_share)
                .delete(delete_share),
        )
        .layer(middleware::from_fn(require_caller))
        .with_state(store)
}

async fn require_caller(req: Request, next: Next) -> Response {
    if req.extensions().get::<Caller>().is_none() {
        return ApiError::unauthorized("no credentials").into_response();
    }
    next.run(req).await
}

fn caller(ext: Option<axum::Extension<Caller>>) -> ApiResult<Caller> {
    ext.map(|axum::Extension(c)| c)
        .ok_or_else(|| ApiError::unauthorized("no credentials"))
}

fn if_match(headers: &HeaderMap) -> ApiResult<Option<u64>> {
    match headers.get(header::IF_MATCH) {
        None => Ok(None),
        Some(v) => {
            let s = v
                .to_str()
                .map_err(|_| ApiError::bad_request("bad If-Match header"))?;
            s.trim()
                .trim_matches('"')
                .parse::<u64>()
                .map(Some)
                .map_err(|_| ApiError::bad_request("If-Match must be a revision number"))
        }
    }
}

fn page(mut items: Vec<Share>, q: &ListQuery) -> Page<Share> {
    let sel = q.label_selector();
    items.retain(|k| {
        let m = k.meta();
        q.name.as_deref().is_none_or(|n| m.name == n)
            && sel.iter().all(|(lk, lv)| m.labels.get(lk) == Some(lv))
    });
    items.sort_by(|a, b| a.meta().id.cmp(&b.meta().id));
    let total = items.len();
    if let Some(l) = q.limit {
        items.truncate(l);
    }
    Page {
        items,
        next: None,
        total,
    }
}

fn shares(store: &pistack_store::Store) -> pistack_store::Collection<Share> {
    store.collection()
}

/// System callers see every project. Everyone else sees only their own,
/// including the empty scope when the token has no project.
fn list_visible(store: &pistack_store::Store, caller: &Caller) -> ApiResult<Vec<Share>> {
    let mut items = shares(store).list(None)?;
    if !caller.system {
        items.retain(|s| s.meta.project == caller.project);
    }
    Ok(items)
}

fn load(store: &pistack_store::Store, caller: &Caller, name_or_id: &str) -> ApiResult<Share> {
    let scope = if caller.system {
        None
    } else {
        caller.project.as_ref()
    };
    let share = shares(store).resolve(scope, name_or_id)?;
    if !caller.system && share.meta.project != caller.project {
        return Err(ApiError::not_found(format!(
            "share {} not found",
            share.meta.id
        )));
    }
    Ok(share)
}

#[derive(Deserialize)]
struct CreateShare {
    #[serde(default)]
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    labels: BTreeMap<String, String>,
    #[serde(default)]
    annotations: BTreeMap<String, String>,
    spec: ShareSpec,
}

#[derive(Deserialize)]
struct UpdateShare {
    #[serde(default)]
    name: Option<String>,
    spec: ShareSpec,
}

async fn list_shares(
    State(store): State<pistack_store::Store>,
    ext: Option<axum::Extension<Caller>>,
    Query(q): Query<ListQuery>,
) -> ApiResult<Json<Page<Share>>> {
    Ok(Json(page(list_visible(&store, &caller(ext)?)?, &q)))
}

async fn create_share(
    State(store): State<pistack_store::Store>,
    ext: Option<axum::Extension<Caller>>,
    Json(req): Json<CreateShare>,
) -> ApiResult<(StatusCode, Json<Share>)> {
    let caller = caller(ext)?;
    let mut spec = req.spec;
    assign_name(&mut spec, Some(&req.name), None)?;
    spec.validate()?;
    let mut share = Share::new(spec.name.clone(), caller.project.clone(), spec);
    share.meta.description = req.description;
    share.meta.labels = req.labels;
    share.meta.annotations = req.annotations;
    realize::apply(&mut share).await?;
    let share = shares(&store).create(share)?;
    Ok((StatusCode::CREATED, Json(share)))
}

async fn get_share(
    State(store): State<pistack_store::Store>,
    ext: Option<axum::Extension<Caller>>,
    Path(id): Path<String>,
) -> ApiResult<Json<Share>> {
    Ok(Json(load(&store, &caller(ext)?, &id)?))
}

async fn update_share(
    State(store): State<pistack_store::Store>,
    ext: Option<axum::Extension<Caller>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(req): Json<UpdateShare>,
) -> ApiResult<Json<Share>> {
    let caller = caller(ext)?;
    let expected = if_match(&headers)?;
    let mut share = load(&store, &caller, &id)?;
    let mut spec = req.spec;
    assign_name(&mut spec, req.name.as_deref(), Some(&share.meta.name))?;
    spec.validate()?;
    share.meta.name = spec.name.clone();
    share.spec = spec;
    realize::apply(&mut share).await?;
    Ok(Json(shares(&store).update(share, expected)?))
}

async fn delete_share(
    State(store): State<pistack_store::Store>,
    ext: Option<axum::Extension<Caller>>,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    let share = load(&store, &caller(ext)?, &id)?;
    realize::remove(&share).await?;
    shares(&store).delete(&share.meta.id)?;
    Ok(StatusCode::NO_CONTENT)
}
