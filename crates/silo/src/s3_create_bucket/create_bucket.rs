// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use crate::s3::{s3_error, BucketQuery};
use axum::{
    extract::{Path, Query, Request, State},
    response::Response,
    Extension,
};
use pistack_types::Caller;

use super::create_bucket_inner::create_bucket_inner;

/// Axum handler for `PUT /{bucket}` (no key). A `CreateBucketConfiguration`
/// body is accepted and ignored.
pub async fn create_bucket(
    State(c): State<Client>,
    ext: Option<Extension<Caller>>,
    Path(bucket): Path<String>,
    Query(q): Query<BucketQuery>,
    req: Request,
) -> Response {
    if q.versioning.is_some() {
        return crate::s3_bucket_versioning::put_handler(c, ext, bucket, req).await;
    }
    match create_bucket_inner(c, ext, bucket, req).await {
        Ok(resp) => resp,
        Err(err) => s3_error(err),
    }
}
