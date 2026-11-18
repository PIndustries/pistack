// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::bucket_name::validate_bucket_name;
use crate::client::Client;
use crate::http::create_project;
use crate::s3::{bucket_exists, caller, ensure_body_hash};
use crate::types::BucketSpec;
use axum::{extract::Request, response::Response, Extension};
use pistack_types::{ApiResult, Caller, Meta, Spec};

use super::location::location;
use super::same_project::same_project;
use super::taken::taken;

pub(crate) async fn create_bucket_inner(
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
