// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use crate::s3::{s3_error, ObjectQuery};
use axum::{
    extract::{Path, Query, Request, State},
    response::{IntoResponse, Response},
    Extension,
};
use pistack_types::Caller;

use super::delete_bucket_inner::delete_bucket_inner;

pub async fn delete_bucket(
    State(c): State<Client>,
    ext: Option<Extension<Caller>>,
    Path(bucket): Path<String>,
    Query(q): Query<ObjectQuery>,
    req: Request,
) -> Response {
    match delete_bucket_inner(c, ext, bucket, q, req).await {
        Ok(status) => status.into_response(),
        Err(err) => s3_error(err),
    }
}
