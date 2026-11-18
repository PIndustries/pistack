// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use crate::http::load_bucket;
use crate::multipart::{self, s3_code_of};
use crate::sigv4::{self, ExpectedPayloadSha256};
use axum::{
    extract::Request,
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Extension,
};
use pistack_types::{ApiError, ApiResult, Caller};

use super::caller::caller;
use super::check_content_md5::check_content_md5;
use super::delete_objects_xml::delete_objects_xml;
use super::deleted_key::DeletedKey;
use super::failed_key::FailedKey;
use super::key_not_there::key_not_there;
use super::s3_max_keys::S3_MAX_KEYS;

pub(crate) async fn delete_objects_inner(
    c: Client,
    ext: Option<Extension<Caller>>,
    bucket_name: String,
    req: Request,
) -> ApiResult<Response> {
    let who = caller(ext)?;
    let bucket = load_bucket(&c, &who, &bucket_name)?;
    // A Content-MD5 that cannot be read is refused before a byte is touched.
    // S3 answers InvalidDigest for a header that is not base64 of 16 bytes
    // and BadDigest for one that is well formed but not the body's hash.
    let declared_md5 = req
        .headers()
        .get_all("content-md5")
        .iter()
        .map(|v| v.to_str().map(str::to_owned).unwrap_or_default())
        .next();
    // The declared body hash is honoured exactly as CompleteMultipartUpload
    // honours it: the SigV4 middleware recorded what the client said the body
    // was, and the request is refused when these are not those bytes.
    let expected = req.extensions().get::<ExpectedPayloadSha256>().cloned();
    let body = axum::body::to_bytes(req.into_body(), 4 * 1024 * 1024)
        .await
        .map_err(|e| ApiError::bad_request(format!("reading delete body: {e}")))?;
    sigv4::verify_body_sha256(expected.as_ref(), &body)?;
    if let Some(declared) = declared_md5 {
        check_content_md5(&declared, &body)?;
    }
    let raw = std::str::from_utf8(&body)
        .map_err(|_| multipart::s3_err("MalformedXML", "body is not UTF-8"))?;
    let parsed = multipart::parse_delete_xml(raw)?;
    if parsed.objects.len() > S3_MAX_KEYS {
        return Err(multipart::s3_err(
            "MalformedXML",
            format!("DeleteObjects takes at most {S3_MAX_KEYS} keys"),
        ));
    }

    let mut deleted: Vec<DeletedKey> = Vec::new();
    let mut failed: Vec<FailedKey> = Vec::new();
    for object in &parsed.objects {
        let outcome = match c.delete_object(&bucket.meta.id, &object.key).await {
            Ok(obj) => Some(DeletedKey {
                key: object.key.clone(),
                // A batch delete that left a marker says so per key, the way
                // it says which generation it named.
                marker: obj.status.delete_marker,
            }),
            // A key that is not there is already as deleted as the caller
            // wants it, so it is not a failure.
            Err(err) if key_not_there(&err) => Some(DeletedKey {
                key: object.key.clone(),
                marker: false,
            }),
            Err(err) => {
                failed.push(FailedKey {
                    key: object.key.clone(),
                    code: s3_code_of(&err).unwrap_or("InternalError").to_owned(),
                    message: err.message,
                });
                None
            }
        };
        let quiet = object.quiet.unwrap_or(parsed.quiet);
        if let Some(entry) = outcome.filter(|_| !quiet) {
            deleted.push(entry);
        }
    }
    Ok((
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/xml")],
        delete_objects_xml(&deleted, &failed),
    )
        .into_response())
}
