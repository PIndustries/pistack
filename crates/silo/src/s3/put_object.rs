// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use axum::{
    extract::{Path, Query, Request, State},
    http::HeaderMap,
    response::Response,
    Extension,
};
use pistack_types::Caller;

use super::object_query::ObjectQuery;
use super::put_object_inner::put_object_inner;
use super::s3_error::s3_error;

pub async fn put_object(
    State(c): State<Client>,
    ext: Option<Extension<Caller>>,
    Path((bucket, key)): Path<(String, String)>,
    Query(q): Query<ObjectQuery>,
    headers: HeaderMap,
    req: Request,
) -> Response {
    match put_object_inner(c, ext, bucket, key, q, headers, req).await {
        Ok(resp) => resp,
        Err(err) => s3_error(err),
    }
}
