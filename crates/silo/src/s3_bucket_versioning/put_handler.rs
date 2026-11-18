// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use crate::s3::s3_error;
use axum::{
    extract::Request,
    response::{IntoResponse, Response},
    Extension,
};
use pistack_types::Caller;

use super::put_versioning::put_versioning;

/// `PUT /{bucket}?versioning`, reached from `s3_create_bucket::create_bucket`.
pub(crate) async fn put_handler(
    c: Client,
    ext: Option<Extension<Caller>>,
    bucket_name: String,
    req: Request,
) -> Response {
    match put_versioning(c, ext, bucket_name, req).await {
        Ok(status) => status.into_response(),
        Err(err) => s3_error(err),
    }
}
