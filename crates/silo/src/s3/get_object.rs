// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use axum::{
    extract::{Path, Query, State},
    http::HeaderMap,
    response::Response,
    Extension,
};
use pistack_types::Caller;

use super::object_query::ObjectQuery;
use super::with_object_read::with_object_read;

pub async fn get_object(
    State(c): State<Client>,
    ext: Option<Extension<Caller>>,
    Path((bucket, key)): Path<(String, String)>,
    Query(q): Query<ObjectQuery>,
    headers: HeaderMap,
) -> Response {
    with_object_read(c, ext, bucket, key, q, headers, true).await
}
