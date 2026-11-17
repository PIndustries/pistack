// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! S3 CreateBucket: `PUT /{bucket}` on the path-style surface.
//!
//! Makes the same bucket record the native `POST /buckets` makes, for the
//! signed caller's project, through the same `Client::create_bucket` (name
//! rules and quota included). The `CreateBucketConfiguration` body is read to
//! honour the declared payload hash, then ignored.

use crate::bucket_name::validate_bucket_name;
use crate::client::Client;
use crate::http::create_project;
use crate::multipart::s3_err;
use crate::s3::{bucket_exists, caller, ensure_body_hash, s3_error};
use crate::types::BucketSpec;
use axum::{
    extract::{Path, Request, State},
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Extension,
};
use pistack_types::{ApiResult, Caller, Meta, Spec};

/// Axum handler for `PUT /{bucket}` (no key). A `CreateBucketConfiguration`
/// body is accepted and ignored.
pub async fn create_bucket(
    State(c): State<Client>,
    ext: Option<Extension<Caller>>,
    Path(bucket): Path<String>,
    req: Request,
) -> Response {
    match create_bucket_inner(c, ext, bucket, req).await {
        Ok(resp) => resp,
        Err(err) => s3_error(err),
    }
}

async fn create_bucket_inner(
    c: Client,
    ext: Option<Extension<Caller>>,
    bucket: String,
    req: Request,
) -> ApiResult<Response> {
    let who = caller(ext)?;
    validate_bucket_name(&bucket)?;
    // Honour the declared payload hash before a record is made; the
    // configuration body itself is not parsed.
    ensure_body_hash(req).await?;
    let project = create_project(&who, &None)?;

    // A name already held is ours to reuse only when it is ours; anything else
    // (another project or the cloud scope) is `BucketAlreadyExists`.
    if let Some(existing) = c.find_bucket(&bucket) {
        return if same_project(&existing, project.as_ref()) {
            location(&bucket)
        } else {
            Err(taken())
        };
    }

    let meta = Meta::new(BucketSpec::KIND, bucket.clone(), project.clone());
    match c.create_bucket(meta, BucketSpec::default()) {
        Ok(_) => location(&bucket),
        // A record for the name appeared between the lookup and the create.
        Err(err) if bucket_exists(&err) => {
            let found = c.find_bucket(&bucket);
            match found.filter(|b| same_project(b, project.as_ref())) {
                Some(_) => location(&bucket),
                None => Err(taken()),
            }
        }
        Err(err) => Err(err),
    }
}

/// The CreateBucket refusal for a name held by someone else.
fn taken() -> pistack_types::ApiError {
    s3_err(
        "BucketAlreadyExists",
        "The requested bucket name is not available. The bucket namespace is shared by all users \
         of the system.",
    )
}

/// `200` with `Location: /<bucket>`, the CreateBucket success answer.
fn location(bucket: &str) -> ApiResult<Response> {
    let mut resp = StatusCode::OK.into_response();
    if let Ok(v) = HeaderValue::from_str(&format!("/{bucket}")) {
        resp.headers_mut().insert(header::LOCATION, v);
    }
    Ok(resp)
}

/// A bucket belongs to the caller only when its project is exactly the
/// caller's. A cloud-scope bucket (no project) is not "ours" to reuse here:
/// an S3 caller always has a project, and reusing another scope's name would
/// hide a real `BucketAlreadyExists`.
fn same_project(existing: &crate::types::Bucket, project: Option<&pistack_types::Id>) -> bool {
    project.is_some() && existing.meta.project.as_ref() == project
}
