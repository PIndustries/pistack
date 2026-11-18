// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use crate::http::load_bucket;
use crate::multipart::s3_err;
use crate::s3::caller;
use crate::sigv4::ExpectedPayloadSha256;
use axum::{extract::Request, http::StatusCode, Extension};
use pistack_types::{ApiError, ApiResult, Caller};

use super::parse_status::parse_status;

pub(crate) async fn put_versioning(
    c: Client,
    ext: Option<Extension<Caller>>,
    bucket_name: String,
    req: Request,
) -> ApiResult<StatusCode> {
    let who = caller(ext)?;
    let bucket = load_bucket(&c, &who, &bucket_name)?;
    // The declared payload hash is honoured before the bucket is touched.
    let expected = req.extensions().get::<ExpectedPayloadSha256>().cloned();
    let body = axum::body::to_bytes(req.into_body(), 1024 * 1024)
        .await
        .map_err(|e| ApiError::bad_request(format!("reading body: {e}")))?;
    crate::sigv4::verify_body_sha256(expected.as_ref(), &body)?;
    let text =
        std::str::from_utf8(&body).map_err(|_| s3_err("MalformedXML", "body is not UTF-8"))?;
    let enabled = parse_status(text)?;
    c.set_versioning(&bucket.meta.id, enabled)?;
    Ok(StatusCode::OK)
}
