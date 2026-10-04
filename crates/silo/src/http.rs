// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! HTTP surface, mounted by the gateway at `/silo`.
//!
//! ```text
//! GET  /                              module info
//! GET|POST        /buckets
//! GET|DELETE      /buckets/{id}
//! GET             /buckets/{id}/objects
//! PUT|GET|DELETE  /buckets/{id}/objects/{*key}
//! ```
//!
//! `{id}` is a bucket id or name. Object puts take a raw body, capped at
//! 1 GiB (axum's default limit is 2 MiB). `{*key}` is the object key and
//! may contain `/`; `..`, empty, and absolute keys are rejected.
//!
//! The `Caller` comes from request extensions. Without one, the request
//! is rejected.

use crate::client::Client;
use crate::types::*;
use axum::{
    body::Body,
    extract::{DefaultBodyLimit, Extension, Path, Query, Request, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use pistack_types::{ApiError, ApiResult, Caller, Id, ListQuery, Meta, Page, Spec};
use serde::Serialize;
use std::collections::BTreeMap;

const DEFAULT_LIMIT: usize = 100;
const MAX_LIMIT: usize = 1000;

pub fn router(client: Client) -> Router {
    Router::new()
        .route("/", get(info))
        .route("/buckets", get(list_buckets).post(create_bucket))
        .route("/buckets/{id}", get(get_bucket).delete(delete_bucket))
        .route("/buckets/{id}/objects", get(list_objects))
        .route(
            "/buckets/{id}/objects/{*key}",
            axum::routing::put(put_object)
                .get(get_object)
                .delete(delete_object)
                .layer(DefaultBodyLimit::max(MAX_OBJECT_BYTES)),
        )
        .fallback(|| async { ApiError::not_found("no such silo endpoint") })
        .layer(middleware::from_fn(require_caller))
        .with_state(client)
}

async fn require_caller(req: Request, next: Next) -> Response {
    if req.extensions().get::<Caller>().is_none() {
        return ApiError::unauthorized("no credentials").into_response();
    }
    next.run(req).await
}

#[derive(Serialize)]
struct Info {
    module: &'static str,
    resources: &'static [&'static str],
}

async fn info() -> Json<Info> {
    Json(Info {
        module: "silo",
        resources: &["bucket", "object"],
    })
}

#[derive(serde::Deserialize)]
struct CreateBucket {
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    labels: BTreeMap<String, String>,
    #[serde(default)]
    annotations: BTreeMap<String, String>,
    #[serde(default)]
    project: Option<Id>,
    #[serde(default)]
    spec: BucketSpec,
}

fn caller(ext: Option<Extension<Caller>>) -> ApiResult<Caller> {
    ext.map(|Extension(c)| c)
        .ok_or_else(|| ApiError::unauthorized("no credentials"))
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

fn visible(caller: &Caller, bucket: Bucket) -> ApiResult<Bucket> {
    let p = &bucket.meta.project;
    if caller.is_admin() || p.is_none() || *p == caller.project {
        Ok(bucket)
    } else {
        Err(ApiError::not_found(format!(
            "bkt {} not found",
            bucket.meta.id
        )))
    }
}

fn load_bucket(client: &Client, caller: &Caller, name_or_id: &str) -> ApiResult<Bucket> {
    if !caller.is_admin() && caller.project.is_none() {
        return Err(ApiError::forbidden("token has no project scope"));
    }
    visible(
        caller,
        client.get_bucket(caller.project.as_ref(), name_or_id)?,
    )
}

fn list_project(caller: &Caller) -> ApiResult<Option<Id>> {
    if caller.is_admin() {
        Ok(caller.project.clone())
    } else {
        caller
            .project
            .clone()
            .map(Some)
            .ok_or_else(|| ApiError::forbidden("token has no project scope"))
    }
}

fn page(mut items: Vec<Bucket>, q: &ListQuery) -> Page<Bucket> {
    let sel = q.label_selector();
    items.retain(|b| {
        q.name.as_deref().is_none_or(|n| b.meta.name == n)
            && sel.iter().all(|(k, v)| b.meta.labels.get(k) == Some(v))
    });
    items.sort_by(|a, b| a.meta.id.cmp(&b.meta.id));
    let total = items.len();
    if let Some(after) = &q.after {
        items.retain(|b| b.meta.id.to_string().as_str() > after.as_str());
    }
    let limit = q.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let next = (items.len() > limit).then(|| items[limit - 1].meta.id.to_string());
    items.truncate(limit);
    Page { items, next, total }
}

fn content_type_header(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

async fn list_buckets(
    State(c): State<Client>,
    ext: Option<Extension<Caller>>,
    Query(q): Query<ListQuery>,
) -> ApiResult<Json<Page<Bucket>>> {
    let project = list_project(&caller(ext)?)?;
    Ok(Json(page(c.list_buckets(project.as_ref())?, &q)))
}

async fn create_bucket(
    State(c): State<Client>,
    ext: Option<Extension<Caller>>,
    Json(body): Json<CreateBucket>,
) -> ApiResult<(StatusCode, Json<Bucket>)> {
    let who = caller(ext)?;
    let project = create_project(&who, &body.project)?;
    let mut meta = Meta::new(BucketSpec::KIND, body.name, project);
    meta.description = body.description;
    meta.labels = body.labels;
    meta.annotations = body.annotations;
    let bucket = c.create_bucket(meta, body.spec)?;
    Ok((StatusCode::CREATED, Json(bucket)))
}

async fn get_bucket(
    State(c): State<Client>,
    ext: Option<Extension<Caller>>,
    Path(id): Path<String>,
) -> ApiResult<Json<Bucket>> {
    Ok(Json(load_bucket(&c, &caller(ext)?, &id)?))
}

async fn delete_bucket(
    State(c): State<Client>,
    ext: Option<Extension<Caller>>,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    let bucket = load_bucket(&c, &caller(ext)?, &id)?;
    c.delete_bucket(&bucket.meta.id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize)]
struct ListedObject {
    id: Id,
    key: String,
    size_bytes: u64,
    sha256: String,
    version: u64,
}

#[derive(Serialize)]
struct ObjectList {
    keys: Vec<String>,
    items: Vec<ListedObject>,
    total: usize,
}

async fn list_objects(
    State(c): State<Client>,
    ext: Option<Extension<Caller>>,
    Path(id): Path<String>,
) -> ApiResult<Json<ObjectList>> {
    let bucket = load_bucket(&c, &caller(ext)?, &id)?;
    let objs = c.list_objects(&bucket.meta.id)?;
    let keys = objs.iter().map(|o| o.spec.key.clone()).collect::<Vec<_>>();
    let items = objs
        .into_iter()
        .map(|o| ListedObject {
            id: o.meta.id,
            key: o.spec.key,
            size_bytes: o.status.size_bytes,
            sha256: o.status.sha256,
            version: o.status.version,
        })
        .collect::<Vec<_>>();
    let total = items.len();
    Ok(Json(ObjectList { keys, items, total }))
}

async fn put_object(
    State(c): State<Client>,
    ext: Option<Extension<Caller>>,
    Path((id, key)): Path<(String, String)>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> ApiResult<(StatusCode, Json<Object>)> {
    let bucket = load_bucket(&c, &caller(ext)?, &id)?;
    let obj = c
        .put_object(&bucket.meta.id, &key, &body, content_type_header(&headers))
        .await?;
    let status = if obj.meta.revision == 1 {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    Ok((status, Json(obj)))
}

async fn get_object(
    State(c): State<Client>,
    ext: Option<Extension<Caller>>,
    Path((id, key)): Path<(String, String)>,
) -> ApiResult<Response> {
    let bucket = load_bucket(&c, &caller(ext)?, &id)?;
    let (obj, bytes) = c.get_object(&bucket.meta.id, &key).await?;
    let mut resp = Body::from(bytes).into_response();
    let headers = resp.headers_mut();
    let ctype = HeaderValue::from_str(&obj.spec.content_type)
        .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream"));
    headers.insert(header::CONTENT_TYPE, ctype);
    if let Ok(etag) = HeaderValue::from_str(&format!("\"{}\"", obj.status.sha256)) {
        headers.insert(header::ETAG, etag);
    }
    Ok(resp)
}

async fn delete_object(
    State(c): State<Client>,
    ext: Option<Extension<Caller>>,
    Path((id, key)): Path<(String, String)>,
) -> ApiResult<StatusCode> {
    let bucket = load_bucket(&c, &caller(ext)?, &id)?;
    c.delete_object(&bucket.meta.id, &key).await?;
    Ok(StatusCode::NO_CONTENT)
}
