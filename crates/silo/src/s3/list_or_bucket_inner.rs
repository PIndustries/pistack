// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use crate::http::load_bucket;
use axum::{
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Extension,
};
use pistack_types::{ApiError, ApiResult, Caller};

use super::caller::caller;
use super::decode_continuation_token::decode_continuation_token;
use super::encode_continuation_token::encode_continuation_token;
use super::list_bucket_page::ListBucketPage;
use super::list_bucket_xml::list_bucket_xml;
use super::list_query::ListQuery;
use super::list_uploads_inner::list_uploads_inner;
use super::parse_encoding_type::parse_encoding_type;
use super::parse_max_keys::parse_max_keys;

pub(crate) async fn list_or_bucket_inner(
    c: Client,
    ext: Option<Extension<Caller>>,
    bucket_name: String,
    q: ListQuery,
) -> ApiResult<Response> {
    if q.uploads.is_some() {
        return list_uploads_inner(c, ext, bucket_name, q).await;
    }
    if q.versioning.is_some() {
        return crate::s3_bucket_versioning::get_versioning(c, ext, bucket_name).await;
    }
    if q.versions.is_some() {
        return crate::s3_list_versions::list_versions(c, ext, bucket_name, q).await;
    }
    if q.list_type.as_deref() != Some("2") {
        return Err(ApiError::bad_request(
            "only ListObjectsV2 (list-type=2) and ListMultipartUploads (?uploads) are supported on this path",
        ));
    }
    let who = caller(ext)?;
    // Auth / visibility first so a garbage token on a foreign bucket is still
    // NoSuchBucket, not InvalidArgument.
    let bucket = load_bucket(&c, &who, &bucket_name)?;
    let encode_url = parse_encoding_type(q.encoding_type.as_deref())?;
    let max_keys = parse_max_keys(q.max_keys.as_deref())?;
    let prefix = q.prefix.as_deref().unwrap_or("");
    let after_owned = if let Some(tok) = q.continuation_token.as_deref().filter(|s| !s.is_empty()) {
        Some(decode_continuation_token(tok)?)
    } else {
        q.start_after
            .as_deref()
            .filter(|s| !s.is_empty())
            .map(|s| s.to_owned())
    };
    let delimiter = q.delimiter.as_deref().filter(|d| !d.is_empty());
    let (items, truncated) = c.list_objects_filtered(
        &bucket.meta.id,
        Some(prefix),
        after_owned.as_deref(),
        max_keys,
        delimiter,
    )?;
    let next = truncated
        .then(|| {
            items
                .last()
                .map(|e| encode_continuation_token(e.sort_key()))
        })
        .flatten();
    let xml = list_bucket_xml(&ListBucketPage {
        bucket: &bucket.meta.name,
        prefix,
        delimiter: delimiter.unwrap_or(""),
        start_after: q.start_after.as_deref().unwrap_or(""),
        max_keys,
        entries: &items,
        truncated,
        next_token: next.as_deref(),
        continuation: q.continuation_token.as_deref(),
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
