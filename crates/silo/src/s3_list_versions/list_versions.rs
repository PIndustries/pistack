// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use crate::client::ListObjectEntry;
use crate::http::load_bucket;
use crate::s3::{caller, parse_encoding_type, parse_max_keys, ListQuery};
use axum::{
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Extension,
};
use pistack_types::{ApiResult, Caller};

use super::versions_page::VersionsPage;
use super::versions_xml::versions_xml;

pub(crate) async fn list_versions(
    c: Client,
    ext: Option<Extension<Caller>>,
    bucket_name: String,
    q: ListQuery,
) -> ApiResult<Response> {
    let who = caller(ext)?;
    // Visibility before argument checks: a garbage marker on a foreign bucket
    // is NoSuchBucket, not InvalidArgument.
    let bucket = load_bucket(&c, &who, &bucket_name)?;
    let encode_url = parse_encoding_type(q.encoding_type.as_deref())?;
    let max_keys = parse_max_keys(q.max_keys.as_deref())?;
    let prefix = q.prefix.as_deref().unwrap_or("");
    let delimiter = q.delimiter.as_deref().filter(|d| !d.is_empty());
    // S3 ignores version-id-marker unless key-marker is set.
    let key_marker = q.key_marker.as_deref().filter(|s| !s.is_empty());
    let version_marker = match key_marker {
        Some(_) => q.version_id_marker.as_deref().filter(|s| !s.is_empty()),
        None => None,
    };
    let (entries, truncated) = c.list_object_versions(
        &bucket.meta.id,
        Some(prefix),
        delimiter,
        key_marker.map(|k| (k, version_marker)),
        max_keys,
    )?;
    // The marker resumes after the last row of this page. Inside a key the
    // version-id marker names the generation just emitted.
    let last = if truncated { entries.last() } else { None };
    let xml = versions_xml(&VersionsPage {
        bucket: &bucket.meta.name,
        prefix,
        delimiter: delimiter.unwrap_or(""),
        key_marker: key_marker.unwrap_or(""),
        version_id_marker: version_marker.unwrap_or(""),
        max_keys,
        entries: &entries,
        truncated,
        next_key: last.map(ListObjectEntry::sort_key),
        next_version: last.and_then(|e| match e {
            ListObjectEntry::Object(o) => Some(o.status.version.to_string()),
            ListObjectEntry::CommonPrefix(_) => None,
        }),
        encode_url,
        // `IsLatest` is the newest generation of the row's key, which the page
        // does not always hold whole — the bucket's history does.
        all: c.list_all_objects(&bucket.meta.id)?,
    });
    let mut resp = (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/xml")],
        xml,
    )
        .into_response();
    resp.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/xml"),
    );
    Ok(resp)
}
