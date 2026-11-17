// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! S3 bucket versioning configuration: `PUT /{bucket}?versioning` and
//! `GET /{bucket}?versioning`.
//!
//! The setting is the one the store already has — `BucketSpec.versioning` —
//! written through [`Client::set_versioning`]. `Enabled` turns it on,
//! `Suspended` turns it off: the store keeps no third state, so a suspended
//! bucket behaves as it did before versioning was ever set, which is what
//! turning it off means here.
//!
//! A GET answers `<VersioningConfiguration/>` with no `<Status>` element when
//! versioning was never set, and with `<Status>Enabled</Status>` once the
//! bucket keeps generations.
//!
//! Both handlers are reached from the existing bucket routes — `GET
//! /{bucket}` and `PUT /{bucket}` notice the `versioning` query and hand over
//! — because axum will not register two routes for one path.

use crate::client::Client;
use crate::http::load_bucket;
use crate::multipart::s3_err;
use crate::s3::{caller, s3_error};
use crate::sigv4::ExpectedPayloadSha256;
use axum::{
    extract::Request,
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Extension,
};
use pistack_types::{ApiError, ApiResult, Caller};

const XMLNS: &str = "http://s3.amazonaws.com/doc/2006-03-01/";

/// The versioning configuration document of a visible bucket.
pub(crate) async fn get_versioning(
    c: Client,
    ext: Option<Extension<Caller>>,
    bucket_name: String,
) -> ApiResult<Response> {
    let who = caller(ext)?;
    let bucket = load_bucket(&c, &who, &bucket_name)?;
    let status = if bucket.spec.versioning {
        "<Status>Enabled</Status>"
    } else {
        ""
    };
    let xml = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
<VersioningConfiguration xmlns=\"{XMLNS}\">{status}</VersioningConfiguration>"
    );
    Ok((
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/xml")],
        xml,
    )
        .into_response())
}

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

async fn put_versioning(
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

/// The `<Status>` of a VersioningConfiguration body: `true` for `Enabled`,
/// `false` for `Suspended`. Anything else is `MalformedXML`.
fn parse_status(body: &str) -> ApiResult<bool> {
    let bad = || {
        s3_err(
            "MalformedXML",
            "VersioningConfiguration needs a Status of Enabled or Suspended",
        )
    };
    if !body.contains("VersioningConfiguration") {
        return Err(bad());
    }
    let inner = body
        .split_once("<Status>")
        .map(|(_, rest)| rest)
        .ok_or_else(bad)?;
    let status = inner
        .split_once("</Status>")
        .map(|(v, _)| v)
        .ok_or_else(bad)?;
    match status.trim() {
        "Enabled" => Ok(true),
        "Suspended" => Ok(false),
        other => Err(s3_err(
            "MalformedXML",
            format!("Status must be Enabled or Suspended, got '{other}'"),
        )),
    }
}
