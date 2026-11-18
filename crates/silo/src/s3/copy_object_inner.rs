// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use crate::copy;
use crate::http::{content_type_header, load_copy_source_bucket};
use crate::multipart::{self};
use axum::{
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use pistack_types::{ApiResult, Caller};
use tokio_util::io::ReaderStream;

use super::content_length::content_length;
use super::header_str::header_str;
use super::metadata_directive::{metadata_directive, MetadataDirective};
use super::xml_escape::xml_escape;

#[allow(clippy::too_many_arguments)]
pub(crate) async fn copy_object_inner(
    c: Client,
    who: &Caller,
    dest_bucket: crate::types::Bucket,
    key: String,
    headers: &HeaderMap,
    copy_raw: &str,
    streaming: bool,
) -> ApiResult<Response> {
    if streaming {
        return Err(multipart::s3_err(
            "InvalidRequest",
            "CopyObject cannot use STREAMING-* payload signing",
        ));
    }
    let wire_len = content_length(headers)?;
    if wire_len.is_some_and(|n| n > 0) {
        return Err(multipart::s3_err(
            "InvalidRequest",
            "CopyObject must not include a request body",
        ));
    }
    if header_str(headers, "x-amz-copy-source-range").is_some() {
        return Err(multipart::s3_err(
            "InvalidArgument",
            "x-amz-copy-source-range is not valid for CopyObject",
        ));
    }

    let src = copy::parse_copy_source(copy_raw)?;
    if src.version_id.is_some() {
        return Err(multipart::s3_err(
            "NotImplemented",
            "object versioning is not supported for CopyObject",
        ));
    }

    // Visibility before any destination write. Foreign or nonexistent
    // source buckets → AccessDenied (do not leak existence).
    let src_bucket = load_copy_source_bucket(&c, who, &src.bucket)?;

    let directive = metadata_directive(headers)?;
    let same_object = src_bucket.meta.id == dest_bucket.meta.id && src.key == key;
    if same_object && directive == MetadataDirective::Copy {
        return Err(multipart::s3_err(
            "InvalidRequest",
            "This copy request is illegal because it is trying to copy an object to itself without changing the object's metadata",
        ));
    }

    let (src_obj, file) = match c.open_object(&src_bucket.meta.id, &src.key).await {
        Ok(v) => v,
        Err(e) if e.status() == StatusCode::NOT_FOUND => {
            return Err(multipart::s3_err(
                "NoSuchKey",
                format!(
                    "The specified key does not exist: {}/{}",
                    src.bucket, src.key
                ),
            ));
        }
        Err(e) => return Err(e),
    };

    copy::check_copy_source_conditions(
        &src_obj,
        header_str(headers, "x-amz-copy-source-if-match"),
        header_str(headers, "x-amz-copy-source-if-none-match"),
        header_str(headers, "x-amz-copy-source-if-unmodified-since"),
        header_str(headers, "x-amz-copy-source-if-modified-since"),
    )?;

    let len = src_obj.status.size_bytes;
    if len > c.max_object_bytes() as u64 {
        return Err(multipart::s3_err(
            "EntityTooLarge",
            format!(
                "object exceeds configured max_object_bytes ({})",
                c.max_object_bytes()
            ),
        ));
    }

    let ctype = match directive {
        MetadataDirective::Copy => Some(src_obj.spec.content_type.clone()),
        MetadataDirective::Replace => Some(crate::types::content_type_or_default(
            content_type_header(headers),
        )),
    };

    // Stream source → temp → atomic rename (same path as PutObject). Safe for
    // same-key REPLACE: the open File keeps the old inode while we write .tmp-*.
    let stream = ReaderStream::new(file);
    let obj = c
        .put_object_stream_sha(&dest_bucket.meta.id, &key, stream, Some(len), ctype, None)
        .await?;
    let etag = if !obj.status.md5.is_empty() {
        obj.status.md5.clone()
    } else {
        obj.status.sha256.clone()
    };
    let last_mod = obj.meta.updated_at.format("%Y-%m-%dT%H:%M:%S%.3fZ");
    let xml = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
<CopyObjectResult xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">\
<LastModified>{last_mod}</LastModified>\
<ETag>&quot;{}&quot;</ETag>\
</CopyObjectResult>",
        xml_escape(&etag)
    );
    let mut resp = (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/xml")],
        xml,
    )
        .into_response();
    if let Ok(v) = HeaderValue::from_str(&format!("\"{etag}\"")) {
        resp.headers_mut().insert(header::ETAG, v);
    }
    Ok(resp)
}
