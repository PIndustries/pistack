// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use axum::{
    extract::{Path, Request, State},
    response::Response,
    Extension,
};
use pistack_types::Caller;

use super::delete_objects_inner::delete_objects_inner;
use super::s3_error::s3_error;

pub async fn delete_objects(
    State(c): State<Client>,
    ext: Option<Extension<Caller>>,
    Path(bucket): Path<String>,
    req: Request,
) -> Response {
    match delete_objects_inner(c, ext, bucket, req).await {
        Ok(resp) => resp,
        Err(err) => s3_error(err),
    }
}
