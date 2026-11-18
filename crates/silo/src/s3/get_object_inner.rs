// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use crate::copy;
use crate::http::load_bucket;
use crate::multipart::{self};
use axum::{
    body::Body,
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Extension,
};
use pistack_types::{ApiResult, Caller};
use std::io::SeekFrom;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio_util::io::ReaderStream;

use super::apply_object_headers::apply_object_headers;
use super::caller::caller;
use super::header_str::header_str;
use super::s3_error::s3_error;
use super::set_version_id::set_version_id;

pub(crate) async fn get_object_inner(
    c: Client,
    ext: Option<Extension<Caller>>,
    bucket: String,
    key: String,
    headers: HeaderMap,
    with_body: bool,
    version: Option<u64>,
) -> ApiResult<Response> {
    let who = caller(ext)?;
    let bucket = load_bucket(&c, &who, &bucket)?;
    // A generation named by `versionId` that turns out to be a delete marker
    // is refused before any file is opened: a marker has no bytes, and S3
    // answers it 405 MethodNotAllowed rather than 404.
    if let Some(v) = version {
        let obj = c.head_version(&bucket.meta.id, &key, v)?;
        if obj.status.delete_marker {
            return Err(crate::delete_marker::not_a_readable_object(&obj));
        }
    }
    if !with_body {
        let obj = match version {
            Some(v) => c.head_version(&bucket.meta.id, &key, v)?,
            None => c.head_object(&bucket.meta.id, &key)?,
        };
        let mut resp = StatusCode::OK.into_response();
        apply_object_headers(resp.headers_mut(), &obj)?;
        set_version_id(resp.headers_mut(), obj.status.version);
        *resp.body_mut() = Body::empty();
        return Ok(resp);
    }
    let (obj, mut file) = match version {
        Some(v) => c.open_version(&bucket.meta.id, &key, v).await?,
        None => c.open_object(&bucket.meta.id, &key).await?,
    };
    let Some(range) = header_str(&headers, "range").and_then(copy::parse_get_range) else {
        let mut resp =
            Body::from_stream(ReaderStream::with_capacity(file, 256 * 1024)).into_response();
        apply_object_headers(resp.headers_mut(), &obj)?;
        set_version_id(resp.headers_mut(), obj.status.version);
        return Ok(resp);
    };
    let size = obj.status.size_bytes;
    let Some((start, end)) = copy::satisfiable(range, size) else {
        // A refused range still says how big the object is.
        let mut resp = s3_error(multipart::s3_err(
            "InvalidRange",
            "The requested range is not satisfiable",
        ));
        *resp.status_mut() = StatusCode::RANGE_NOT_SATISFIABLE;
        if let Ok(v) = HeaderValue::from_str(&format!("bytes */{size}")) {
            resp.headers_mut().insert(header::CONTENT_RANGE, v);
        }
        return Ok(resp);
    };
    // A range that names the whole object — an empty one included — is
    // answered as a plain 200 without a Content-Range, as S3 answers it.
    if start == 0 && end + 1 >= size {
        let mut resp =
            Body::from_stream(ReaderStream::with_capacity(file, 256 * 1024)).into_response();
        apply_object_headers(resp.headers_mut(), &obj)?;
        set_version_id(resp.headers_mut(), obj.status.version);
        return Ok(resp);
    }
    let len = end - start + 1;
    if start > 0 {
        file.seek(SeekFrom::Start(start))
            .await
            .map_err(multipart::io_err)?;
    }
    let mut resp =
        Body::from_stream(ReaderStream::with_capacity(file.take(len), 256 * 1024)).into_response();
    apply_object_headers(resp.headers_mut(), &obj)?;
    set_version_id(resp.headers_mut(), obj.status.version);
    *resp.status_mut() = StatusCode::PARTIAL_CONTENT;
    if let Ok(v) = HeaderValue::from_str(&len.to_string()) {
        resp.headers_mut().insert(header::CONTENT_LENGTH, v);
    }
    if let Ok(v) = HeaderValue::from_str(&format!("bytes {start}-{end}/{size}")) {
        resp.headers_mut().insert(header::CONTENT_RANGE, v);
    }
    Ok(resp)
}
