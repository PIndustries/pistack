// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! S3 ListObjectVersions: `GET /{bucket}?versions`.
//!
//! Lists every generation the bucket holds: the current one of each key plus
//! every older generation a versioned bucket has kept. A `<Version>` carries
//! `Key`, `VersionId` (the record's generation number as a decimal string),
//! `IsLatest`, `LastModified`, `ETag` and `Size`. Paging follows the S3
//! shape: `prefix`, `max-keys`, and `key-marker` with `version-id-marker` to
//! resume inside one key; the answer names where the next page starts.
//!
//! The store keeps no delete marker — a key with no record left is simply not
//! listed, and no `<DeleteMarker>` element appears.

use crate::client::Client;
use crate::client::ListObjectEntry;
use crate::http::load_bucket;
use crate::s3::{
    caller, parse_encoding_type, parse_max_keys, xml_escape, xml_list_value, ListQuery,
};
use axum::{
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Extension,
};
use pistack_types::{ApiResult, Caller};

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

struct VersionsPage<'a> {
    bucket: &'a str,
    prefix: &'a str,
    delimiter: &'a str,
    key_marker: &'a str,
    version_id_marker: &'a str,
    max_keys: usize,
    entries: &'a [ListObjectEntry],
    truncated: bool,
    next_key: Option<&'a str>,
    next_version: Option<String>,
    encode_url: bool,
}

fn versions_xml(page: &VersionsPage<'_>) -> String {
    let enc = page.encode_url;
    let mut body = String::new();
    body.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>");
    body.push_str("<ListVersionsResult xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">");
    body.push_str(&format!("<Name>{}</Name>", xml_escape(page.bucket)));
    body.push_str(&format!(
        "<Prefix>{}</Prefix>",
        xml_list_value(page.prefix, enc)
    ));
    if !page.delimiter.is_empty() {
        body.push_str(&format!(
            "<Delimiter>{}</Delimiter>",
            xml_list_value(page.delimiter, enc)
        ));
    }
    body.push_str(&format!(
        "<KeyMarker>{}</KeyMarker>",
        xml_list_value(page.key_marker, enc)
    ));
    body.push_str(&format!(
        "<VersionIdMarker>{}</VersionIdMarker>",
        xml_escape(page.version_id_marker)
    ));
    if let Some(key) = page.next_key {
        body.push_str(&format!(
            "<NextKeyMarker>{}</NextKeyMarker>",
            xml_list_value(key, enc)
        ));
        body.push_str(&format!(
            "<NextVersionIdMarker>{}</NextVersionIdMarker>",
            xml_escape(page.next_version.as_deref().unwrap_or(""))
        ));
    }
    body.push_str(&format!("<MaxKeys>{}</MaxKeys>", page.max_keys));
    if enc {
        body.push_str("<EncodingType>url</EncodingType>");
    }
    body.push_str(&format!(
        "<IsTruncated>{}</IsTruncated>",
        if page.truncated { "true" } else { "false" }
    ));
    for entry in page.entries {
        match entry {
            ListObjectEntry::Object(obj) => {
                let obj = obj.as_ref();
                let etag = if !obj.status.md5.is_empty() {
                    &obj.status.md5
                } else {
                    &obj.status.sha256
                };
                body.push_str("<Version>");
                body.push_str(&format!(
                    "<Key>{}</Key>",
                    xml_list_value(&obj.spec.key, enc)
                ));
                body.push_str(&format!("<VersionId>{}</VersionId>", obj.status.version));
                body.push_str(&format!(
                    "<IsLatest>{}</IsLatest>",
                    if obj.status.latest { "true" } else { "false" }
                ));
                body.push_str(&format!(
                    "<LastModified>{}</LastModified>",
                    obj.meta.updated_at.format("%Y-%m-%dT%H:%M:%S%.3fZ")
                ));
                body.push_str(&format!("<ETag>&quot;{}&quot;</ETag>", xml_escape(etag)));
                body.push_str(&format!("<Size>{}</Size>", obj.status.size_bytes));
                body.push_str("<StorageClass>STANDARD</StorageClass>");
                body.push_str("</Version>");
            }
            ListObjectEntry::CommonPrefix(cp) => {
                body.push_str("<CommonPrefixes><Prefix>");
                body.push_str(&xml_list_value(cp, enc));
                body.push_str("</Prefix></CommonPrefixes>");
            }
        }
    }
    body.push_str("</ListVersionsResult>");
    body
}
