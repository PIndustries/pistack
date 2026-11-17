// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! S3 DeleteBucket: `DELETE /{bucket}` (no `uploadId`) on the path-style
//! surface.
//!
//! Removes the same record the native `DELETE /buckets/{id}` removes, through
//! the same visibility check (`load_bucket`) and the same
//! `Client::delete_bucket`. The S3 difference is that a bucket holding
//! objects or unfinished multipart uploads is refused (`BucketNotEmpty`)
//! instead of taken down with its contents.

use crate::client::Client;
use crate::http::load_bucket;
use crate::multipart::s3_err;
use crate::s3::{caller, ensure_body_hash, s3_error, ObjectQuery};
use axum::{
    extract::{Path, Query, Request, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Extension,
};
use pistack_types::{ApiResult, Caller};

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

async fn delete_bucket_inner(
    c: Client,
    ext: Option<Extension<Caller>>,
    bucket: String,
    q: ObjectQuery,
    req: Request,
) -> ApiResult<StatusCode> {
    let who = caller(ext)?;
    let bucket = load_bucket(&c, &who, &bucket)?;
    ensure_body_hash(req).await?;
    if q.upload_id.is_some() {
        // An object-path abort carries a key; there is none here.
        return Err(s3_err("NoSuchKey", "no object key on this request"));
    }
    if c.list_objects(&bucket.meta.id)?.is_empty() && !c.has_staged_uploads(&bucket.meta.id).await?
    {
        // Nothing a listing shows. `delete_bucket` has the last word: a
        // versioned bucket can still be held open by a generation under a
        // delete marker, which no listing shows.
        return c
            .delete_bucket(&bucket.meta.id)
            .await
            .map(|_| StatusCode::NO_CONTENT);
    }
    Err(s3_err(
        "BucketNotEmpty",
        "The bucket you tried to delete is not empty",
    ))
}
