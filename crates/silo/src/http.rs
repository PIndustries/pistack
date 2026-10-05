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
//!
//! S3 path-style (same router, same auth):
//! PUT|GET|HEAD|DELETE|POST  /{bucket}/{*key}  (POST: multipart create/complete)
//! GET                       /{bucket}?list-type=2&…
//! GET                       /{bucket}?uploads&…  (ListMultipartUploads)
//! ```
//!
//! `{id}` is a bucket id or name. Object puts stream the body to disk and
//! refuse anything larger than the client's configured max (default 5 GiB).
//! `{*key}` is the object key and may contain `/`; `..`, empty, and absolute
//! keys are rejected.
//!
//! The `Caller` comes from request extensions (gate bearer token) or from
//! AWS SigV4 verification ([`crate::sigv4`]) on S3 routes. Without one, the
//! request is rejected.

use crate::client::Client;
use crate::s3;
use crate::sigv4::{self, SigV4State};
use crate::types::*;
use axum::{
    body::Body,
    extract::{DefaultBodyLimit, Extension, FromRef, Path, Query, Request, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use futures::TryStreamExt;
use pistack_types::{ApiError, ApiResult, Caller, Id, ListQuery, Meta, Page, Spec};
use serde::Serialize;
use std::collections::BTreeMap;
use tokio_util::io::ReaderStream;

const DEFAULT_LIMIT: usize = 100;
const MAX_LIMIT: usize = 1000;

/// Router state: silo client plus optional keep client for SigV4.
#[derive(Clone)]
pub struct AppState {
    pub client: Client,
    pub sigv4: SigV4State,
}

impl FromRef<AppState> for Client {
    fn from_ref(s: &AppState) -> Client {
        s.client.clone()
    }
}

impl FromRef<AppState> for SigV4State {
    fn from_ref(s: &AppState) -> SigV4State {
        s.sigv4.clone()
    }
}

/// Native + S3 routes with bearer auth only (no SigV4). Used by unit tests
/// that inject a [`Caller`] via request extensions.
pub fn router(client: Client) -> Router {
    router_with(client, None, DEFAULT_REGION.to_owned())
}

/// Full router: bearer token and (when `keep` is set) AWS SigV4 via keep
/// access keys. `region` is the SigV4 region name ([`SiloConfig::region`]).
pub fn router_with(client: Client, keep: Option<keep::Client>, region: String) -> Router {
    let state = AppState {
        client,
        sigv4: SigV4State { keep, region },
    };
    // Object puts enforce the size cap while streaming; disable axum's
    // default 2 MiB buffer limit the same way mold does for image uploads.
    let object_routes = axum::routing::put(put_object)
        .get(get_object)
        .delete(delete_object)
        .layer(DefaultBodyLimit::disable());
    let s3_object = axum::routing::put(s3::put_object)
        .get(s3::get_object)
        .post(s3::post_object)
        .delete(s3::delete_object)
        .head(s3::head_object)
        .layer(DefaultBodyLimit::disable());

    Router::new()
        .route("/", get(info))
        .route("/buckets", get(list_buckets).post(create_bucket))
        .route("/buckets/{id}", get(get_bucket).delete(delete_bucket))
        .route("/buckets/{id}/objects", get(list_objects))
        .route("/buckets/{id}/objects/{*key}", object_routes)
        .route("/{bucket}", get(s3::list_or_bucket))
        .route("/{bucket}/{*key}", s3_object)
        .fallback(|| async { ApiError::not_found("no such silo endpoint") })
        // Inner: demand a Caller. Outer: SigV4 may attach one first.
        .layer(middleware::from_fn(require_caller))
        .layer(middleware::from_fn_with_state(
            state.sigv4.clone(),
            sigv4::middleware,
        ))
        .with_state(state)
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

pub(crate) fn load_bucket(client: &Client, caller: &Caller, name_or_id: &str) -> ApiResult<Bucket> {
    if !caller.is_admin() && caller.project.is_none() {
        return Err(ApiError::forbidden("token has no project scope"));
    }
    visible(
        caller,
        client.get_bucket(caller.project.as_ref(), name_or_id)?,
    )
}

/// Resolve a copy-source bucket for UploadPartCopy.
///
/// Foreign-project and nonexistent names both yield S3 `AccessDenied` so
/// bucket existence is not leaked. A visible bucket is returned for the
/// subsequent key lookup (`NoSuchKey` stays distinct).
pub(crate) fn load_copy_source_bucket(
    client: &Client,
    caller: &Caller,
    name_or_id: &str,
) -> ApiResult<Bucket> {
    match load_bucket(client, caller, name_or_id) {
        Ok(b) => Ok(b),
        Err(_) => Err(crate::multipart::s3_err("AccessDenied", "Access Denied")),
    }
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

pub(crate) fn content_type_header(headers: &HeaderMap) -> Option<String> {
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
    req: Request,
) -> ApiResult<(StatusCode, Json<Object>)> {
    let bucket = load_bucket(&c, &caller(ext)?, &id)?;
    let len = headers
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok());
    let stream = req
        .into_body()
        .into_data_stream()
        .map_err(std::io::Error::other);
    let obj = c
        .put_object_stream(
            &bucket.meta.id,
            &key,
            stream,
            len,
            content_type_header(&headers),
        )
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
    let (obj, file) = c.open_object(&bucket.meta.id, &key).await?;
    let mut resp = Body::from_stream(ReaderStream::with_capacity(file, 256 * 1024)).into_response();
    let headers = resp.headers_mut();
    let ctype = HeaderValue::from_str(&obj.spec.content_type)
        .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream"));
    headers.insert(header::CONTENT_TYPE, ctype);
    if let Ok(len) = HeaderValue::from_str(&obj.status.size_bytes.to_string()) {
        headers.insert(header::CONTENT_LENGTH, len);
    }
    let etag_hex = if !obj.status.md5.is_empty() {
        obj.status.md5.clone()
    } else {
        obj.status.sha256.clone()
    };
    if let Ok(etag) = HeaderValue::from_str(&format!("\"{etag_hex}\"")) {
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
