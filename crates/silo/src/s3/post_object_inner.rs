// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use crate::http::{content_type_header, load_bucket};
use crate::multipart::{self};
use crate::sigv4::{self, ExpectedPayloadSha256};
use crate::types::*;
use axum::{
    extract::Request,
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Extension,
};
use pistack_types::{ApiError, ApiResult, Caller};

use super::caller::caller;
use super::object_query::ObjectQuery;
use super::xml_escape::xml_escape;

pub(crate) async fn post_object_inner(
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
    let expected = req.extensions().get::<ExpectedPayloadSha256>().cloned();
    if q.uploads.is_some() && q.upload_id.is_none() {
        // CreateMultipartUpload: after load_bucket, before staging write.
        reject_xml_unsafe_key(&key)?;
        let body = axum::body::to_bytes(req.into_body(), 1024 * 1024)
            .await
            .map_err(|e| ApiError::bad_request(format!("reading body: {e}")))?;
        sigv4::verify_body_sha256(expected.as_ref(), &body)?;
        let ctype = content_type_header(&headers);
        let upload_id = c
            .create_multipart_upload(&bucket.meta.id, &key, ctype)
            .await?;
        let xml = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
<InitiateMultipartUploadResult xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">\
<Bucket>{}</Bucket><Key>{}</Key><UploadId>{}</UploadId>\
</InitiateMultipartUploadResult>",
            xml_escape(&bucket.meta.name),
            xml_escape(&key),
            xml_escape(&upload_id)
        );
        return Ok((
            StatusCode::OK,
            [(header::CONTENT_TYPE, "application/xml")],
            xml,
        )
            .into_response());
    }
    let Some(upload_id) = q.upload_id.as_deref() else {
        return Err(ApiError::bad_request(
            "POST on object requires ?uploads or ?uploadId=",
        ));
    };
    let body = axum::body::to_bytes(req.into_body(), 4 * 1024 * 1024)
        .await
        .map_err(|e| ApiError::bad_request(format!("reading complete body: {e}")))?;
    sigv4::verify_body_sha256(expected.as_ref(), &body)?;
    let body = std::str::from_utf8(&body)
        .map_err(|_| multipart::s3_err("MalformedXML", "body is not UTF-8"))?;
    let parts = multipart::parse_complete_xml(body)?;
    let (obj, etag) = c
        .complete_multipart_upload(&bucket.meta.id, &key, upload_id, &parts)
        .await?;
    let xml = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
<CompleteMultipartUploadResult xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">\
<Location>/{}/{}</Location><Bucket>{}</Bucket><Key>{}</Key>\
<ETag>&quot;{}&quot;</ETag></CompleteMultipartUploadResult>",
        xml_escape(&bucket.meta.name),
        xml_escape(&obj.spec.key),
        xml_escape(&bucket.meta.name),
        xml_escape(&obj.spec.key),
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
