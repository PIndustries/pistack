// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use axum::{http::HeaderMap, response::Response, Extension};
use pistack_types::Caller;

use super::get_object_inner::get_object_inner;
use super::list_parts_inner::list_parts_inner;
use super::object_query::ObjectQuery;
use super::s3_error::s3_error;
use super::set_delete_marker::set_delete_marker;
use super::set_version_id::set_version_id;

/// GET and HEAD on an object path, and the marker headers on both.
///
/// A key under a delete marker is not there, and a marker named by
/// `versionId` is not an object; either way the answer says so with
/// `x-amz-delete-marker: true` and the marker's version, as S3 does.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn with_object_read(
    c: Client,
    ext: Option<Extension<Caller>>,
    bucket: String,
    key: String,
    q: ObjectQuery,
    headers: HeaderMap,
    with_body: bool,
) -> Response {
    if let Some(upload_id) = q.upload_id.as_deref() {
        return match list_parts_inner(c, ext, bucket, key, upload_id).await {
            Ok(resp) => resp,
            Err(err) => s3_error(err),
        };
    }
    let version = match q.version_id.as_deref() {
        Some(raw) => match crate::versions::parse_version_id(raw) {
            Ok(v) => Some(v),
            Err(err) => return s3_error(err),
        },
        None => None,
    };
    match get_object_inner(c, ext, bucket, key, headers, with_body, version).await {
        Ok(resp) => resp,
        Err(err) => {
            let marker = crate::delete_marker::marker_version_of(&err);
            let mut resp = s3_error(err);
            if let Some(version) = marker {
                set_delete_marker(resp.headers_mut());
                set_version_id(resp.headers_mut(), version);
            }
            resp
        }
    }
}
