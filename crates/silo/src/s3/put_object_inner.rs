// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::chunked::StreamingAuth;
use crate::client::Client;
use crate::http::{content_type_header, load_bucket};
use crate::multipart::{self};
use crate::sigv4::ExpectedPayloadSha256;
use crate::types::*;
use axum::{
    extract::Request,
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Extension,
};
use pistack_types::{ApiResult, Caller};

use super::caller::caller;
use super::content_length::content_length;
use super::copy_object_inner::copy_object_inner;
use super::etag_value::etag_value;
use super::header_str::header_str;
use super::maybe_decode_chunked::maybe_decode_chunked;
use super::object_query::ObjectQuery;
use super::set_version_id::set_version_id;
use super::upload_part_copy_inner::upload_part_copy_inner;

pub(crate) async fn put_object_inner(
    c: Client,
    ext: Option<Extension<Caller>>,
    bucket: String,
    key: String,
    q: ObjectQuery,
    headers: HeaderMap,
    req: Request,
) -> ApiResult<Response> {
    let who = caller(ext)?;
    let bucket = load_bucket(&c, &who, &bucket)?;
    let wire_len = content_length(&headers)?;
    let expected_sha = req
        .extensions()
        .get::<ExpectedPayloadSha256>()
        .map(|e| e.0.clone());
    let streaming = req.extensions().get::<StreamingAuth>().cloned();
    if let Some(ref s) = streaming {
        if s.decoded_content_length > c.max_object_bytes() as u64 {
            return Err(multipart::s3_err(
                "EntityTooLarge",
                format!(
                    "object exceeds configured max_object_bytes ({})",
                    c.max_object_bytes()
                ),
            ));
        }
    }
    let object_len = streaming
        .as_ref()
        .map(|s| Some(s.decoded_content_length))
        .unwrap_or(wire_len);

    let copy_raw = header_str(&headers, "x-amz-copy-source");
    if let Some(copy_raw) = copy_raw {
        match (q.upload_id.as_deref(), q.part_number.as_deref()) {
            (Some(upload_id), Some(part_raw)) => {
                return upload_part_copy_inner(
                    c,
                    &who,
                    bucket,
                    key,
                    upload_id,
                    part_raw,
                    &headers,
                    copy_raw,
                    streaming.is_some(),
                )
                .await;
            }
            (None, None) => {
                // CopyObject destination key (after load_bucket above).
                reject_xml_unsafe_key(&key)?;
                return copy_object_inner(
                    c,
                    &who,
                    bucket,
                    key,
                    &headers,
                    copy_raw,
                    streaming.is_some(),
                )
                .await;
            }
            _ => {
                return Err(multipart::s3_err(
                    "InvalidArgument",
                    "UploadPartCopy requires both partNumber and uploadId",
                ));
            }
        }
    }

    if let (Some(upload_id), Some(part_raw)) = (q.upload_id.as_deref(), q.part_number.as_deref()) {
        let part_number: u32 = part_raw
            .parse()
            .map_err(|_| multipart::s3_err("InvalidArgument", "partNumber must be a number"))?;
        let stream = maybe_decode_chunked(req.into_body().into_data_stream(), streaming)?;
        let etag = c
            .upload_part(
                &bucket.meta.id,
                &key,
                upload_id,
                part_number,
                stream,
                object_len,
                expected_sha.as_deref(),
            )
            .await?;
        let mut resp = StatusCode::OK.into_response();
        if let Ok(v) = HeaderValue::from_str(&format!("\"{etag}\"")) {
            resp.headers_mut().insert(header::ETAG, v);
        }
        resp.headers_mut()
            .insert(header::CONTENT_LENGTH, HeaderValue::from_static("0"));
        return Ok(resp);
    }
    // PutObject (incl. aws-chunked streaming); after load_bucket, before write.
    reject_xml_unsafe_key(&key)?;
    let ctype = content_type_header(&headers);
    let stream = maybe_decode_chunked(req.into_body().into_data_stream(), streaming)?;
    let obj = c
        .put_object_stream_sha(
            &bucket.meta.id,
            &key,
            stream,
            object_len,
            ctype,
            expected_sha.as_deref(),
        )
        .await?;
    let etag = etag_value(&obj);
    let mut resp = StatusCode::OK.into_response();
    if let Some(v) = etag {
        resp.headers_mut().insert(header::ETAG, v);
    }
    if bucket.spec.versioning {
        set_version_id(resp.headers_mut(), obj.status.version);
    }
    resp.headers_mut()
        .insert(header::CONTENT_LENGTH, HeaderValue::from_static("0"));
    Ok(resp)
}
