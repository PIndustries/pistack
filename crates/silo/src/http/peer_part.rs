// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use axum::{
    body::Body,
    extract::{Path, State},
    http::{header, HeaderMap, HeaderValue},
    response::{IntoResponse, Response},
};
use pistack_types::ApiResult;
use tokio_util::io::ReaderStream;

use super::bucket_id::bucket_id;
use super::peer_allowed::peer_allowed;

pub(crate) async fn peer_part(
    State(c): State<Client>,
    headers: HeaderMap,
    Path((bucket, upload, part)): Path<(String, String, u32)>,
) -> ApiResult<Response> {
    peer_allowed(&c, &headers)?;
    let (file, size) = c
        .open_staged_part(&bucket_id(&bucket)?, &upload, part)
        .await?;
    let mut resp = Body::from_stream(ReaderStream::with_capacity(file, 256 * 1024)).into_response();
    if let Ok(len) = HeaderValue::from_str(&size.to_string()) {
        resp.headers_mut().insert(header::CONTENT_LENGTH, len);
    }
    Ok(resp)
}
