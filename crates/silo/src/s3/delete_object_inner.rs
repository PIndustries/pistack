// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use crate::http::load_bucket;
use crate::sigv4::{self, ExpectedPayloadSha256};
use axum::{
    extract::Request,
    http::StatusCode,
    response::{IntoResponse, Response},
    Extension,
};
use pistack_types::{ApiError, ApiResult, Caller};

use super::caller::caller;
use super::object_query::ObjectQuery;
use super::set_delete_marker::set_delete_marker;
use super::set_version_id::set_version_id;

pub(crate) async fn delete_object_inner(
    c: Client,
    ext: Option<Extension<Caller>>,
    bucket: String,
    key: String,
    q: ObjectQuery,
    req: Request,
) -> ApiResult<Response> {
    let who = caller(ext)?;
    let bucket = load_bucket(&c, &who, &bucket)?;
    let expected = req.extensions().get::<ExpectedPayloadSha256>().cloned();
    if expected.is_some() {
        let body = axum::body::to_bytes(req.into_body(), 1024 * 1024)
            .await
            .map_err(|e| ApiError::bad_request(format!("reading body: {e}")))?;
        sigv4::verify_body_sha256(expected.as_ref(), &body)?;
    }
    if let Some(upload_id) = q.upload_id.as_deref() {
        c.abort_multipart_upload(&bucket.meta.id, &key, upload_id)
            .await?;
        return Ok(StatusCode::NO_CONTENT.into_response());
    }
    // A key already hidden and a key the call is about to hide both answer
    // with a marker, so the route asks what was there before it deletes.
    let hidden_before = c
        .head_object(&bucket.meta.id, &key)
        .err()
        .is_some_and(|e| crate::delete_marker::marker_version_of(&e).is_some());
    let obj = match q.version_id.as_deref() {
        Some(raw) => {
            let version = crate::versions::parse_version_id(raw)?;
            c.delete_version(&bucket.meta.id, &key, version).await?
        }
        None => c.delete_object(&bucket.meta.id, &key).await?,
    };
    let mut resp = StatusCode::NO_CONTENT.into_response();
    // `x-amz-version-id` names the generation the call acted on: the marker it
    // wrote, the survivor it promoted, or the generation it took.
    // `x-amz-delete-marker` says the key is now hidden behind a marker. A
    // delete that named a `versionId` never hides a key: it removed one
    // generation, and the survivor is current or the key is gone. A plain
    // delete hides a key only over bytes that were there — one that found the
    // key already hidden takes it whole and answers no marker.
    set_version_id(resp.headers_mut(), obj.status.version);
    if q.version_id.is_none() && !hidden_before && obj.status.delete_marker {
        set_delete_marker(resp.headers_mut());
    }
    Ok(resp)
}
