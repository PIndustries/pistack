// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! HTTP surface, mounted by the gateway at `/image`.
//!
//! `POST /images` makes the record, `PUT /images/{id}/data` streams the
//! bytes in, `GET /images/{id}/data` streams them out. Everything is
//! filtered by the caller's visibility.

use axum::{
    body::Body,
    extract::{DefaultBodyLimit, Path, Query, Request, State},
    http::{header, HeaderMap, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Extension, Json, Router,
};
use futures::TryStreamExt;
use pistack_types::{ApiError, ApiResult, Caller, Id, Kind, ListQuery, Page};
use serde::Deserialize;
use tokio_util::io::{ReaderStream, StreamReader};

use crate::client::{Client, CreateImage, UpdateImage};
use crate::types::*;

pub fn router(client: Client) -> Router {
    Router::new()
        .route("/images", get(list_images).post(create_image))
        .route(
            "/images/{id}",
            get(get_image).put(update_image).delete(delete_image),
        )
        .route(
            "/images/{id}/data",
            get(download).put(upload).layer(DefaultBodyLimit::disable()),
        )
        .route("/images/{id}/import", post(import_image))
        .route("/images/{id}/members", get(list_members).post(add_member))
        .route(
            "/images/{id}/members/{project}",
            axum::routing::delete(remove_member),
        )
        .layer(middleware::from_fn(require_caller))
        .with_state(client)
}

async fn require_caller(req: Request, next: Next) -> Response {
    if req.extensions().get::<Caller>().is_none() {
        return ApiError::unauthorized("no credentials").into_response();
    }
    next.run(req).await
}

/// The gateway attaches a `Caller`. Without one the request is refused.
fn caller(ext: Option<Extension<Caller>>) -> ApiResult<Caller> {
    ext.map(|Extension(c)| c)
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

fn editable(c: &Client, caller: &Caller, id: &str) -> ApiResult<Image> {
    let img = c.get_visible(caller, id)?;
    if !c.can_edit(caller, &img) {
        return Err(ApiError::forbidden(format!(
            "image {} belongs to another project",
            img.meta.id
        )));
    }
    Ok(img)
}

// ---- images ---------------------------------------------------------------

async fn list_images(
    State(c): State<Client>,
    ext: Option<Extension<Caller>>,
    Query(q): Query<ListQuery>,
) -> ApiResult<Json<Page<Image>>> {
    Ok(Json(page(c.list_visible(&caller(ext)?)?, &q)))
}

async fn create_image(
    State(c): State<Client>,
    ext: Option<Extension<Caller>>,
    Json(mut req): Json<CreateImage>,
) -> ApiResult<(StatusCode, Json<Image>)> {
    let caller = caller(ext)?;
    match (&req.project, &caller.project) {
        (Some(p), Some(own)) if p != own && !caller.is_admin() => {
            return Err(ApiError::forbidden(
                "cannot create images in another project",
            ));
        }
        (None, own) => req.project = own.clone(),
        _ => {}
    }
    if req.spec.visibility == Visibility::Public && !caller.is_admin() {
        return Err(ApiError::forbidden("only admins publish public images"));
    }
    Ok((StatusCode::CREATED, Json(c.create(req)?)))
}

async fn get_image(
    State(c): State<Client>,
    ext: Option<Extension<Caller>>,
    Path(id): Path<String>,
) -> ApiResult<Json<Image>> {
    Ok(Json(c.get_visible(&caller(ext)?, &id)?))
}

async fn update_image(
    State(c): State<Client>,
    ext: Option<Extension<Caller>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(req): Json<UpdateImage>,
) -> ApiResult<Json<Image>> {
    let caller = caller(ext)?;
    let img = editable(&c, &caller, &id)?;
    if req.visibility == Some(Visibility::Public) && !caller.is_admin() {
        return Err(ApiError::forbidden("only admins publish public images"));
    }
    Ok(Json(c.update(&img.meta.id, req, if_match(&headers)?)?))
}

async fn delete_image(
    State(c): State<Client>,
    ext: Option<Extension<Caller>>,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    let img = editable(&c, &caller(ext)?, &id)?;
    c.delete(&img.meta.id).await?;
    Ok(StatusCode::NO_CONTENT)
}

// ---- data -----------------------------------------------------------------

async fn upload(
    State(c): State<Client>,
    ext: Option<Extension<Caller>>,
    Path(id): Path<String>,
    req: Request,
) -> ApiResult<Json<Image>> {
    let img = editable(&c, &caller(ext)?, &id)?;
    let stream = req
        .into_body()
        .into_data_stream()
        .map_err(std::io::Error::other);
    let reader = StreamReader::new(stream);
    Ok(Json(c.upload(&img.meta.id, Box::pin(reader)).await?))
}

async fn download(
    State(c): State<Client>,
    ext: Option<Extension<Caller>>,
    Path(id): Path<String>,
) -> ApiResult<Response> {
    let img = c.get_visible(&caller(ext)?, &id)?;
    let data = c.open_image(&img).await?;
    let mut resp = Body::from_stream(ReaderStream::with_capacity(data, 256 * 1024)).into_response();
    let h = resp.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        "application/octet-stream".parse().map_err(internal)?,
    );
    if let Some(n) = img.status.size_bytes {
        h.insert(
            header::CONTENT_LENGTH,
            n.to_string().parse().map_err(internal)?,
        );
    }
    if let Some(s) = &img.status.sha256 {
        h.insert("x-image-sha256", s.parse().map_err(internal)?);
    }
    h.insert(
        header::CONTENT_DISPOSITION,
        format!(
            "attachment; filename=\"{}.{}\"",
            img.meta.name, img.spec.format
        )
        .parse()
        .map_err(internal)?,
    );
    Ok(resp)
}

fn internal<E: std::fmt::Display>(e: E) -> ApiError {
    ApiError::internal(e.to_string())
}

#[derive(Deserialize)]
struct ImportBody {
    url: String,
}

async fn import_image(
    State(c): State<Client>,
    ext: Option<Extension<Caller>>,
    Path(id): Path<String>,
    Json(body): Json<ImportBody>,
) -> ApiResult<Json<Image>> {
    let img = editable(&c, &caller(ext)?, &id)?;
    Ok(Json(c.import(&img.meta.id, &body.url).await?))
}

// ---- members --------------------------------------------------------------

async fn list_members(
    State(c): State<Client>,
    ext: Option<Extension<Caller>>,
    Path(id): Path<String>,
) -> ApiResult<Json<Page<ImageMember>>> {
    let img = c.get_visible(&caller(ext)?, &id)?;
    let items = c.list_members(&img.meta.id)?;
    let total = items.len();
    Ok(Json(Page {
        items,
        next: None,
        total,
    }))
}

#[derive(Deserialize)]
struct MemberBody {
    project: Id,
}

async fn add_member(
    State(c): State<Client>,
    ext: Option<Extension<Caller>>,
    Path(id): Path<String>,
    Json(body): Json<MemberBody>,
) -> ApiResult<(StatusCode, Json<ImageMember>)> {
    let img = editable(&c, &caller(ext)?, &id)?;
    if body.project.kind != "proj" {
        return Err(ApiError::bad_request("project must be a proj-… id"));
    }
    Ok((
        StatusCode::CREATED,
        Json(c.add_member(&img.meta.id, body.project)?),
    ))
}

async fn remove_member(
    State(c): State<Client>,
    ext: Option<Extension<Caller>>,
    Path((id, project)): Path<(String, String)>,
) -> ApiResult<StatusCode> {
    let img = editable(&c, &caller(ext)?, &id)?;
    let project = Id::parse("proj", &project)
        .ok_or_else(|| ApiError::bad_request(format!("'{project}' is not a project id")))?;
    c.remove_member(&img.meta.id, &project)?;
    Ok(StatusCode::NO_CONTENT)
}
