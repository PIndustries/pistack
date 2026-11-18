// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use axum::{
    body::Body,
    extract::{Extension, Path, State},
    http::{header, HeaderValue},
    response::{IntoResponse, Response},
};
use pistack_types::{ApiResult, Caller};
use tokio_util::io::ReaderStream;

use super::caller::caller;
use super::load_bucket::load_bucket;

pub(crate) async fn get_object(
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
