// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use crate::http::load_bucket;
use axum::{
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Extension,
};
use pistack_types::{ApiResult, Caller};

use super::caller::caller;
use super::list_query::ListQuery;
use super::list_uploads_page::ListUploadsPage;
use super::list_uploads_xml::list_uploads_xml;
use super::parse_encoding_type::parse_encoding_type;
use super::parse_limit::parse_limit;
use super::s3_max_uploads::S3_MAX_UPLOADS;

pub(crate) async fn list_uploads_inner(
    c: Client,
    ext: Option<Extension<Caller>>,
    bucket_name: String,
    q: ListQuery,
) -> ApiResult<Response> {
    let who = caller(ext)?;
    let bucket = load_bucket(&c, &who, &bucket_name)?;
    let encode_url = parse_encoding_type(q.encoding_type.as_deref())?;
    let max_uploads = parse_limit(q.max_uploads.as_deref(), "max-uploads", S3_MAX_UPLOADS)?;
    let prefix = q.prefix.as_deref().unwrap_or("");
    let key_marker = q.key_marker.as_deref().unwrap_or("");
    // S3 ignores upload-id-marker unless key-marker is set.
    let upload_id_marker = if key_marker.is_empty() {
        ""
    } else {
        q.upload_id_marker.as_deref().unwrap_or("")
    };
    let delimiter = q.delimiter.as_deref().filter(|d| !d.is_empty());
    let (entries, truncated) = c
        .list_multipart_uploads(
            &bucket.meta.id,
            Some(prefix),
            Some(key_marker),
            Some(upload_id_marker),
            max_uploads,
            delimiter,
        )
        .await?;
    let xml = list_uploads_xml(&ListUploadsPage {
        bucket: &bucket.meta.name,
        prefix,
        key_marker,
        upload_id_marker,
        delimiter: delimiter.unwrap_or(""),
        max_uploads,
        entries: &entries,
        truncated,
        encode_url,
    });
    Ok((
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/xml")],
        xml,
    )
        .into_response())
}
