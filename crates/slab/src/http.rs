// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! HTTP surface, mounted by the gateway at `/volume`.
//!
//! Handlers validate, resolve the caller's project scope and call `Client`.
//! They never touch `status` themselves.

use axum::{
    extract::{Path, Query, Request, State},
    http::{HeaderMap, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Extension, Json, Router,
};
use pistack_types::{ApiError, ApiResult, Caller, Id, Kind, ListQuery, Page};
use serde::Deserialize;

use crate::client::{Client, Create, UpdateMeta};
use crate::types::*;

pub fn router(client: Client) -> Router {
    Router::new()
        .route(
            "/volume-types",
            get(list_volume_types).post(create_volume_type),
        )
        .route(
            "/volume-types/{id}",
            get(get_volume_type)
                .put(update_volume_type)
                .delete(delete_volume_type),
        )
        .route("/volumes", get(list_volumes).post(create_volume))
        .route(
            "/volumes/{id}",
            get(get_volume).put(update_volume).delete(delete_volume),
        )
        .route("/volumes/{id}/attach", post(attach_volume))
        .route("/volumes/{id}/detach", post(detach_volume))
        .route("/volumes/{id}/extend", post(extend_volume))
        .route("/snapshots", get(list_snapshots).post(create_snapshot))
        .route("/snapshots/{id}", get(get_snapshot).delete(delete_snapshot))
        .route("/backups", get(list_backups).post(create_backup))
        .route("/backups/{id}", get(get_backup).delete(delete_backup))
        .route("/backups/{id}/restore", post(restore_backup))
        .layer(middleware::from_fn(require_caller))
        .with_state(client)
}

async fn require_caller(req: Request, next: Next) -> Response {
    if req.extensions().get::<Caller>().is_none() {
        return ApiError::unauthorized("no credentials").into_response();
    }
    next.run(req).await
}

// ---- caller scope ---------------------------------------------------------

/// The gateway attaches a `Caller`. Without one the request is refused.
fn caller(ext: Option<Extension<Caller>>) -> ApiResult<Caller> {
    ext.map(|Extension(c)| c)
        .ok_or_else(|| ApiError::unauthorized("no credentials"))
}

/// Project a list/get is scoped to: admins see everything unless they
/// carry a project; everyone else sees only their project.
fn scope(c: &Caller) -> Option<Id> {
    c.project.clone()
}

/// Project a create lands in. Admins may name one; others get their own.
fn target_project(c: &Caller, requested: Option<Id>) -> ApiResult<Option<Id>> {
    match (requested, &c.project) {
        (Some(p), Some(own)) if &p != own && !c.is_admin() => Err(ApiError::forbidden(
            "cannot create resources in another project",
        )),
        (Some(p), _) => Ok(Some(p)),
        (None, own) => Ok(own.clone()),
    }
}

/// Non-admins may only touch their own project's resources.
fn check_visible<K: Kind>(c: &Caller, item: &K) -> ApiResult<()> {
    if c.is_admin() && c.project.is_none() {
        return Ok(());
    }
    if item.meta().project == c.project {
        return Ok(());
    }
    Err(ApiError::not_found(format!(
        "{} {} not found",
        K::KIND,
        item.meta().id
    )))
}

fn if_match(headers: &HeaderMap) -> ApiResult<Option<u64>> {
    match headers.get("if-match") {
        None => Ok(None),
        Some(v) => {
            let s = v
                .to_str()
                .map_err(|_| ApiError::bad_request("bad If-Match header"))?;
            let s = s.trim().trim_matches('"');
            s.parse::<u64>()
                .map(Some)
                .map_err(|_| ApiError::bad_request("If-Match must be a revision number"))
        }
    }
}

fn page<K: Kind>(items: Vec<K>, q: &ListQuery) -> Page<K> {
    let sel = q.label_selector();
    let mut items: Vec<K> = items
        .into_iter()
        .filter(|k| {
            sel.iter()
                .all(|(lk, lv)| k.meta().labels.get(lk) == Some(lv))
        })
        .filter(|k| q.name.as_deref().is_none_or(|n| k.meta().name == n))
        .collect();
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

fn created<T: serde::Serialize>(t: T) -> impl IntoResponse {
    (StatusCode::CREATED, Json(t))
}

// ---- volume types ---------------------------------------------------------

async fn list_volume_types(
    State(c): State<Client>,
    Query(q): Query<ListQuery>,
) -> ApiResult<Json<Page<VolumeType>>> {
    Ok(Json(page(c.list_volume_types()?, &q)))
}

async fn create_volume_type(
    State(c): State<Client>,
    ext: Option<Extension<Caller>>,
    Json(req): Json<Create<VolumeTypeSpec>>,
) -> ApiResult<impl IntoResponse> {
    if !caller(ext)?.is_admin() {
        return Err(ApiError::forbidden("only admins manage volume types"));
    }
    Ok(created(c.create_volume_type(req)?))
}

async fn get_volume_type(
    State(c): State<Client>,
    Path(id): Path<String>,
) -> ApiResult<Json<VolumeType>> {
    Ok(Json(c.get_volume_type(&id)?))
}

#[derive(Deserialize)]
struct UpdateVolumeType {
    #[serde(flatten)]
    meta: UpdateMeta,
    #[serde(default)]
    spec: Option<VolumeTypeSpec>,
}

async fn update_volume_type(
    State(c): State<Client>,
    ext: Option<Extension<Caller>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(req): Json<UpdateVolumeType>,
) -> ApiResult<Json<VolumeType>> {
    if !caller(ext)?.is_admin() {
        return Err(ApiError::forbidden("only admins manage volume types"));
    }
    let vt = c.get_volume_type(&id)?;
    Ok(Json(c.update_volume_type(
        &vt.meta.id,
        req.meta,
        req.spec,
        if_match(&headers)?,
    )?))
}

async fn delete_volume_type(
    State(c): State<Client>,
    ext: Option<Extension<Caller>>,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    if !caller(ext)?.is_admin() {
        return Err(ApiError::forbidden("only admins manage volume types"));
    }
    let vt = c.get_volume_type(&id)?;
    c.delete_volume_type(&vt.meta.id)?;
    Ok(StatusCode::NO_CONTENT)
}

// ---- volumes --------------------------------------------------------------

async fn list_volumes(
    State(c): State<Client>,
    ext: Option<Extension<Caller>>,
    Query(q): Query<ListQuery>,
) -> ApiResult<Json<Page<Volume>>> {
    let caller = caller(ext)?;
    Ok(Json(page(c.list_volumes(scope(&caller).as_ref())?, &q)))
}

async fn create_volume(
    State(c): State<Client>,
    ext: Option<Extension<Caller>>,
    Json(mut req): Json<Create<VolumeSpec>>,
) -> ApiResult<impl IntoResponse> {
    let caller = caller(ext)?;
    req.project = target_project(&caller, req.project.take())?;
    Ok(created(c.create_volume(req).await?))
}

fn load_volume(c: &Client, caller: &Caller, id: &str) -> ApiResult<Volume> {
    let v = c.get_volume(scope(caller).as_ref(), id)?;
    check_visible(caller, &v)?;
    Ok(v)
}

async fn get_volume(
    State(c): State<Client>,
    ext: Option<Extension<Caller>>,
    Path(id): Path<String>,
) -> ApiResult<Json<Volume>> {
    Ok(Json(load_volume(&c, &caller(ext)?, &id)?))
}

async fn update_volume(
    State(c): State<Client>,
    ext: Option<Extension<Caller>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(req): Json<UpdateMeta>,
) -> ApiResult<Json<Volume>> {
    let v = load_volume(&c, &caller(ext)?, &id)?;
    Ok(Json(c.update_volume(
        &v.meta.id,
        req,
        if_match(&headers)?,
    )?))
}

async fn delete_volume(
    State(c): State<Client>,
    ext: Option<Extension<Caller>>,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    let v = load_volume(&c, &caller(ext)?, &id)?;
    c.request_delete_volume(&v.meta.id)?;
    Ok(StatusCode::ACCEPTED)
}

#[derive(Deserialize)]
struct AttachBody {
    machine: Id,
    #[serde(default)]
    device: Option<String>,
}

async fn attach_volume(
    State(c): State<Client>,
    ext: Option<Extension<Caller>>,
    Path(id): Path<String>,
    Json(body): Json<AttachBody>,
) -> ApiResult<Json<Volume>> {
    let v = load_volume(&c, &caller(ext)?, &id)?;
    if body.machine.kind != "mach" {
        return Err(ApiError::bad_request("machine must be a mach-… id"));
    }
    let device = if c.has_queue() {
        Some(
            c.plug_volume(&v.meta.id, &body.machine, body.device)
                .await?,
        )
    } else {
        body.device
    };
    Ok(Json(c.mark_attached(&v.meta.id, &body.machine, device)?))
}

async fn detach_volume(
    State(c): State<Client>,
    ext: Option<Extension<Caller>>,
    Path(id): Path<String>,
) -> ApiResult<Json<Volume>> {
    let v = load_volume(&c, &caller(ext)?, &id)?;
    if c.has_queue() {
        if let Some(machine) = v.status.attached_to.clone() {
            c.unplug_volume(&v.meta.id, &machine, v.status.device.clone())
                .await?;
        }
    }
    Ok(Json(c.mark_detached(&v.meta.id)?))
}

#[derive(Deserialize)]
struct ExtendBody {
    size_gib: u64,
}

async fn extend_volume(
    State(c): State<Client>,
    ext: Option<Extension<Caller>>,
    Path(id): Path<String>,
    Json(body): Json<ExtendBody>,
) -> ApiResult<(StatusCode, Json<Volume>)> {
    let v = load_volume(&c, &caller(ext)?, &id)?;
    Ok((
        StatusCode::ACCEPTED,
        Json(c.extend_volume(&v.meta.id, body.size_gib).await?),
    ))
}

// ---- snapshots ------------------------------------------------------------

async fn list_snapshots(
    State(c): State<Client>,
    ext: Option<Extension<Caller>>,
    Query(q): Query<ListQuery>,
) -> ApiResult<Json<Page<Snapshot>>> {
    let caller = caller(ext)?;
    Ok(Json(page(c.list_snapshots(scope(&caller).as_ref())?, &q)))
}

async fn create_snapshot(
    State(c): State<Client>,
    ext: Option<Extension<Caller>>,
    Json(req): Json<Create<SnapshotSpec>>,
) -> ApiResult<impl IntoResponse> {
    let caller = caller(ext)?;
    // The snapshot inherits its volume's project; the volume must be visible.
    let vol = c.get_volume(None, &req.spec.volume.to_string())?;
    check_visible(&caller, &vol)?;
    Ok(created(c.create_snapshot(req).await?))
}

fn load_snapshot(c: &Client, caller: &Caller, id: &str) -> ApiResult<Snapshot> {
    let s = c.get_snapshot(scope(caller).as_ref(), id)?;
    check_visible(caller, &s)?;
    Ok(s)
}

async fn get_snapshot(
    State(c): State<Client>,
    ext: Option<Extension<Caller>>,
    Path(id): Path<String>,
) -> ApiResult<Json<Snapshot>> {
    Ok(Json(load_snapshot(&c, &caller(ext)?, &id)?))
}

async fn delete_snapshot(
    State(c): State<Client>,
    ext: Option<Extension<Caller>>,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    let s = load_snapshot(&c, &caller(ext)?, &id)?;
    c.request_delete_snapshot(&s.meta.id)?;
    Ok(StatusCode::ACCEPTED)
}

// ---- backups --------------------------------------------------------------

async fn list_backups(
    State(c): State<Client>,
    ext: Option<Extension<Caller>>,
    Query(q): Query<ListQuery>,
) -> ApiResult<Json<Page<Backup>>> {
    let caller = caller(ext)?;
    Ok(Json(page(c.list_backups(scope(&caller).as_ref())?, &q)))
}

async fn create_backup(
    State(c): State<Client>,
    ext: Option<Extension<Caller>>,
    Json(req): Json<Create<BackupSpec>>,
) -> ApiResult<impl IntoResponse> {
    let caller = caller(ext)?;
    let vol = c.get_volume(None, &req.spec.volume.to_string())?;
    check_visible(&caller, &vol)?;
    Ok(created(c.create_backup(req).await?))
}

fn load_backup(c: &Client, caller: &Caller, id: &str) -> ApiResult<Backup> {
    let b = c.get_backup(scope(caller).as_ref(), id)?;
    check_visible(caller, &b)?;
    Ok(b)
}

async fn get_backup(
    State(c): State<Client>,
    ext: Option<Extension<Caller>>,
    Path(id): Path<String>,
) -> ApiResult<Json<Backup>> {
    Ok(Json(load_backup(&c, &caller(ext)?, &id)?))
}

async fn delete_backup(
    State(c): State<Client>,
    ext: Option<Extension<Caller>>,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    let b = load_backup(&c, &caller(ext)?, &id)?;
    c.request_delete_backup(&b.meta.id)?;
    Ok(StatusCode::ACCEPTED)
}

async fn restore_backup(
    State(c): State<Client>,
    ext: Option<Extension<Caller>>,
    Path(id): Path<String>,
) -> ApiResult<Json<Volume>> {
    let b = load_backup(&c, &caller(ext)?, &id)?;
    Ok(Json(c.restore_backup(&b.meta.id)?))
}
