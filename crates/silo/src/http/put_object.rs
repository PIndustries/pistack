// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use crate::types::*;
use axum::{
    extract::{Extension, Path, Request, State},
    http::{header, HeaderMap, StatusCode},
    Json,
};
use futures::TryStreamExt;
use pistack_types::{ApiResult, Caller};

use super::caller::caller;
use super::content_type_header::content_type_header;
use super::load_bucket::load_bucket;

pub(crate) async fn put_object(
    State(c): State<Client>,
    ext: Option<Extension<Caller>>,
    Path((id, key)): Path<(String, String)>,
    headers: HeaderMap,
    req: Request,
) -> ApiResult<(StatusCode, Json<Object>)> {
    let bucket = load_bucket(&c, &caller(ext)?, &id)?;
    // After visibility; before any write / temp file.
    reject_xml_unsafe_key(&key)?;
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
