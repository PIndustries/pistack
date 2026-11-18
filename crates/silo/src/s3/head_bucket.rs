// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use axum::{
    extract::{Path, State},
    response::{IntoResponse, Response},
    Extension,
};
use pistack_types::Caller;

use super::head_bucket_inner::head_bucket_inner;
use super::s3_error::s3_error;

/// HeadBucket: `HEAD /{bucket}`. 200 when the bucket is visible to the
/// caller, 404 (`NoSuchBucket`) otherwise — the same visibility rule as every
/// other bucket route, with no body.
pub async fn head_bucket(
    State(c): State<Client>,
    ext: Option<Extension<Caller>>,
    Path(bucket): Path<String>,
) -> Response {
    match head_bucket_inner(c, ext, bucket).await {
        Ok(status) => status.into_response(),
        Err(err) => s3_error(err),
    }
}
