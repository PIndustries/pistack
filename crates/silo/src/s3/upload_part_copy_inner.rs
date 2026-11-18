// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use crate::copy;
use crate::http::load_copy_source_bucket;
use crate::multipart::{self};
use axum::{
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use chrono::Utc;
use pistack_types::{ApiResult, Caller};
use std::io::SeekFrom;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio_util::io::ReaderStream;

use super::content_length::content_length;
use super::header_str::header_str;
use super::xml_escape::xml_escape;

#[allow(clippy::too_many_arguments)]
pub(crate) async fn upload_part_copy_inner(
    c: Client,
    who: &Caller,
    dest_bucket: crate::types::Bucket,
    key: String,
    upload_id: &str,
    part_raw: &str,
    headers: &HeaderMap,
    copy_raw: &str,
    streaming: bool,
) -> ApiResult<Response> {
    if streaming {
        return Err(multipart::s3_err(
            "InvalidRequest",
            "UploadPartCopy cannot use STREAMING-* payload signing",
        ));
    }
    let wire_len = content_length(headers)?;
    if wire_len.is_some_and(|n| n > 0) {
        return Err(multipart::s3_err(
            "InvalidRequest",
            "UploadPartCopy must not include a request body",
        ));
    }

    let src = copy::parse_copy_source(copy_raw)?;
    if src.version_id.is_some() {
        return Err(multipart::s3_err(
            "NotImplemented",
            "object versioning is not supported for UploadPartCopy",
        ));
    }
    let part_number: u32 = part_raw
        .parse()
        .map_err(|_| multipart::s3_err("InvalidArgument", "partNumber must be a number"))?;

    // Visibility before any dest part file is opened/created. Foreign or
    // nonexistent source buckets → AccessDenied (do not leak existence).
    let src_bucket = load_copy_source_bucket(&c, who, &src.bucket)?;
    let (src_obj, mut file) = match c.open_object(&src_bucket.meta.id, &src.key).await {
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

    let (offset, len) = match header_str(headers, "x-amz-copy-source-range") {
        Some(raw) => copy::parse_copy_source_range(raw, src_obj.status.size_bytes)?,
        None => (0, src_obj.status.size_bytes),
    };
    if len > c.max_object_bytes() as u64 {
        return Err(multipart::s3_err(
            "EntityTooLarge",
            format!(
                "copied part exceeds configured max_object_bytes ({})",
                c.max_object_bytes()
            ),
        ));
    }
    if offset > 0 || len < src_obj.status.size_bytes {
        file.seek(SeekFrom::Start(offset))
            .await
            .map_err(multipart::io_err)?;
    }
    let limited = file.take(len);
    let stream = ReaderStream::new(limited);
    let etag = c
        .upload_part(
            &dest_bucket.meta.id,
            &key,
            upload_id,
            part_number,
            stream,
            Some(len),
            None,
        )
        .await?;
    let last_mod = Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ");
    let xml = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
<CopyPartResult xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">\
<LastModified>{last_mod}</LastModified>\
<ETag>&quot;{}&quot;</ETag>\
</CopyPartResult>",
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
