// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use axum::{
    extract::{Path, Query, State},
    response::Response,
    Extension,
};
use pistack_types::Caller;

use super::list_or_bucket_inner::list_or_bucket_inner;
use super::list_query::ListQuery;
use super::s3_error::s3_error;

pub async fn list_or_bucket(
    State(c): State<Client>,
    ext: Option<Extension<Caller>>,
    Path(bucket): Path<String>,
    Query(q): Query<ListQuery>,
) -> Response {
    match list_or_bucket_inner(c, ext, bucket, q).await {
        Ok(resp) => resp,
        Err(err) => s3_error(err),
    }
}
