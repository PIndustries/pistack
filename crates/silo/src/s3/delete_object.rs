// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use axum::{
    extract::{Path, Query, Request, State},
    response::{IntoResponse, Response},
    Extension,
};
use pistack_types::Caller;

use super::delete_object_inner::delete_object_inner;
use super::object_query::ObjectQuery;
use super::s3_error::s3_error;

pub async fn delete_object(
    State(c): State<Client>,
    ext: Option<Extension<Caller>>,
    Path((bucket, key)): Path<(String, String)>,
    Query(q): Query<ObjectQuery>,
    req: Request,
) -> Response {
    match delete_object_inner(c, ext, bucket, key, q, req).await {
        Ok(resp) => resp.into_response(),
        Err(err) => s3_error(err),
    }
}
