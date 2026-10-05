// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Path-style S3-compatible routes on the silo router.
//!
//! ```text
//! PUT|GET|HEAD|DELETE|POST  /{bucket}/{*key}
//! GET                       /{bucket}?list-type=2&prefix=&continuation-token=&max-keys=
//! GET                       /{bucket}?uploads&prefix=&key-marker=&upload-id-marker=&max-uploads=
//!                                     ListMultipartUploads
//!
//! Multipart (query on the object path):
//! POST   ?uploads                         CreateMultipartUpload
//! PUT    ?partNumber=N&uploadId=X         UploadPart
//! POST   ?uploadId=X  (XML body)          CompleteMultipartUpload
//! DELETE ?uploadId=X                      AbortMultipartUpload
//! GET    ?uploadId=X                      ListParts
//! ```
//!
//! Auth: gate bearer `Caller` (request extensions) or AWS Signature
//! Version 4 ([`crate::sigv4`]) via keep access keys. Header SigV4 and
//! presigned GET/PUT URLs are supported; `STREAMING-AWS4-HMAC-SHA256-PAYLOAD`
//! (aws-chunked) returns `NotImplemented`. Single-PUT ETag is the content
//! MD5 hex (quoted); completed multipart ETag is `{md5-of-part-md5s}-{n}`
//! (quoted).
//!
//! Staging dirs under `.uploads/` are removed on abort and complete, and
//! by the [`crate::gc`] sweep once older than `multipart_ttl_secs`.
//! ListMultipartUploads does not support `delimiter` (no CommonPrefixes)
//! and does not report Initiator/Owner.

use crate::client::Client;
use crate::http::{content_type_header, load_bucket};
use crate::multipart::{self, s3_code_of};
use crate::sigv4::ExpectedPayloadSha256;
use crate::types::*;
use axum::{
    body::Body,
    extract::{Path, Query, Request, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Extension,
};
use pistack_types::{ApiError, ApiResult, Caller};
use serde::Deserialize;
use tokio_util::io::ReaderStream;

const S3_MAX_KEYS: usize = 1000;
const S3_MAX_UPLOADS: usize = 1000;

#[derive(Debug, Deserialize, Default)]
pub struct ObjectQuery {
    /// Presence of `uploads` (even empty) means CreateMultipartUpload on POST.
    pub uploads: Option<String>,
    #[serde(rename = "uploadId")]
    pub upload_id: Option<String>,
    #[serde(rename = "partNumber")]
    pub part_number: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
pub struct ListQuery {
    #[serde(rename = "list-type")]
    pub list_type: Option<String>,
    pub prefix: Option<String>,
    #[serde(rename = "continuation-token")]
    pub continuation_token: Option<String>,
    #[serde(rename = "max-keys")]
    pub max_keys: Option<String>,
    #[serde(rename = "start-after")]
    pub start_after: Option<String>,
    /// Presence of `uploads` (even empty) means ListMultipartUploads.
    pub uploads: Option<String>,
    #[serde(rename = "key-marker")]
    pub key_marker: Option<String>,
    #[serde(rename = "upload-id-marker")]
    pub upload_id_marker: Option<String>,
    #[serde(rename = "max-uploads")]
    pub max_uploads: Option<String>,
    pub delimiter: Option<String>,
}

pub async fn put_object(
    State(c): State<Client>,
    ext: Option<Extension<Caller>>,
    Path((bucket, key)): Path<(String, String)>,
    Query(q): Query<ObjectQuery>,
    headers: HeaderMap,
    req: Request,
) -> Response {
    match put_object_inner(c, ext, bucket, key, q, headers, req).await {
        Ok(resp) => resp,
        Err(err) => s3_error(err),
    }
}

async fn put_object_inner(
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
    let len = content_length(&headers)?;
    let expected_sha = req
        .extensions()
        .get::<ExpectedPayloadSha256>()
        .map(|e| e.0.clone());
    if let (Some(upload_id), Some(part_raw)) = (q.upload_id.as_deref(), q.part_number.as_deref()) {
        let part_number: u32 = part_raw
            .parse()
            .map_err(|_| multipart::s3_err("InvalidArgument", "partNumber must be a number"))?;
        let stream = req.into_body().into_data_stream();
        let etag = c
            .upload_part(&bucket.meta.id, &key, upload_id, part_number, stream, len)
            .await?;
        let _ = expected_sha; // parts: prefer UNSIGNED-PAYLOAD until part hashing lands
        let mut resp = StatusCode::OK.into_response();
        if let Ok(v) = HeaderValue::from_str(&format!("\"{etag}\"")) {
            resp.headers_mut().insert(header::ETAG, v);
        }
        resp.headers_mut()
            .insert(header::CONTENT_LENGTH, HeaderValue::from_static("0"));
        return Ok(resp);
    }
    let ctype = content_type_header(&headers);
    let stream = req.into_body().into_data_stream();
    let obj = c
        .put_object_stream_sha(
            &bucket.meta.id,
            &key,
            stream,
            len,
            ctype,
            expected_sha.as_deref(),
        )
        .await?;
    let etag = etag_value(&obj);
    let mut resp = StatusCode::OK.into_response();
    if let Some(v) = etag {
        resp.headers_mut().insert(header::ETAG, v);
    }
    resp.headers_mut()
        .insert(header::CONTENT_LENGTH, HeaderValue::from_static("0"));
    Ok(resp)
}

pub async fn get_object(
    State(c): State<Client>,
    ext: Option<Extension<Caller>>,
    Path((bucket, key)): Path<(String, String)>,
    Query(q): Query<ObjectQuery>,
) -> Response {
    if let Some(upload_id) = q.upload_id.as_deref() {
        return match list_parts_inner(c, ext, bucket, key, upload_id).await {
            Ok(resp) => resp,
            Err(err) => s3_error(err),
        };
    }
    match get_object_inner(c, ext, bucket, key, true).await {
        Ok(resp) => resp,
        Err(err) => s3_error(err),
    }
}

pub async fn head_object(
    State(c): State<Client>,
    ext: Option<Extension<Caller>>,
    Path((bucket, key)): Path<(String, String)>,
) -> Response {
    match get_object_inner(c, ext, bucket, key, false).await {
        Ok(resp) => resp,
        Err(err) => s3_error(err),
    }
}

async fn get_object_inner(
    c: Client,
    ext: Option<Extension<Caller>>,
    bucket: String,
    key: String,
    with_body: bool,
) -> ApiResult<Response> {
    let who = caller(ext)?;
    let bucket = load_bucket(&c, &who, &bucket)?;
    if with_body {
        let (obj, file) = c.open_object(&bucket.meta.id, &key).await?;
        let mut resp =
            Body::from_stream(ReaderStream::with_capacity(file, 256 * 1024)).into_response();
        apply_object_headers(resp.headers_mut(), &obj)?;
        Ok(resp)
    } else {
        let obj = c.head_object(&bucket.meta.id, &key)?;
        let mut resp = StatusCode::OK.into_response();
        apply_object_headers(resp.headers_mut(), &obj)?;
        *resp.body_mut() = Body::empty();
        Ok(resp)
    }
}

pub async fn delete_object(
    State(c): State<Client>,
    ext: Option<Extension<Caller>>,
    Path((bucket, key)): Path<(String, String)>,
    Query(q): Query<ObjectQuery>,
) -> Response {
    match delete_object_inner(c, ext, bucket, key, q).await {
        Ok(status) => status.into_response(),
        Err(err) => s3_error(err),
    }
}

async fn delete_object_inner(
    c: Client,
    ext: Option<Extension<Caller>>,
    bucket: String,
    key: String,
    q: ObjectQuery,
) -> ApiResult<StatusCode> {
    let who = caller(ext)?;
    let bucket = load_bucket(&c, &who, &bucket)?;
    if let Some(upload_id) = q.upload_id.as_deref() {
        c.abort_multipart_upload(&bucket.meta.id, &key, upload_id)
            .await?;
        return Ok(StatusCode::NO_CONTENT);
    }
    c.delete_object(&bucket.meta.id, &key).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn post_object(
    State(c): State<Client>,
    ext: Option<Extension<Caller>>,
    Path((bucket, key)): Path<(String, String)>,
    Query(q): Query<ObjectQuery>,
    headers: HeaderMap,
    req: Request,
) -> Response {
    match post_object_inner(c, ext, bucket, key, q, headers, req).await {
        Ok(resp) => resp,
        Err(err) => s3_error(err),
    }
}

async fn post_object_inner(
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
    if q.uploads.is_some() && q.upload_id.is_none() {
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

async fn list_parts_inner(
    c: Client,
    ext: Option<Extension<Caller>>,
    bucket: String,
    key: String,
    upload_id: &str,
) -> ApiResult<Response> {
    let who = caller(ext)?;
    let bucket = load_bucket(&c, &who, &bucket)?;
    let (_meta, parts) = c.list_parts(&bucket.meta.id, &key, upload_id).await?;
    let mut xml = String::new();
    xml.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>");
    xml.push_str("<ListPartsResult xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">");
    xml.push_str(&format!(
        "<Bucket>{}</Bucket><Key>{}</Key><UploadId>{}</UploadId>",
        xml_escape(&bucket.meta.name),
        xml_escape(&key),
        xml_escape(upload_id)
    ));
    xml.push_str("<PartNumberMarker>0</PartNumberMarker>");
    xml.push_str(&format!(
        "<NextPartNumberMarker>{}</NextPartNumberMarker>",
        parts.last().map(|p| p.part_number).unwrap_or(0)
    ));
    xml.push_str("<MaxParts>1000</MaxParts><IsTruncated>false</IsTruncated>");
    for p in &parts {
        xml.push_str("<Part>");
        xml.push_str(&format!("<PartNumber>{}</PartNumber>", p.part_number));
        xml.push_str(&format!("<ETag>&quot;{}&quot;</ETag>", xml_escape(&p.etag)));
        xml.push_str(&format!("<Size>{}</Size>", p.size));
        xml.push_str("</Part>");
    }
    xml.push_str("</ListPartsResult>");
    Ok((
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/xml")],
        xml,
    )
        .into_response())
}

pub async fn list_or_bucket(
    State(c): State<Client>,
    ext: Option<Extension<Caller>>,
    Path(bucket): Path<String>,
    Query(q): Query<ListQuery>,
) -> Response {
    match list_or_bucket_inner(c, ext, bucket, q).await {
        Ok(resp) => resp,
        Err(err) => s3_error(err),
    }
}

async fn list_or_bucket_inner(
    c: Client,
    ext: Option<Extension<Caller>>,
    bucket_name: String,
    q: ListQuery,
) -> ApiResult<Response> {
    if q.uploads.is_some() {
        return list_uploads_inner(c, ext, bucket_name, q).await;
    }
    if q.list_type.as_deref() != Some("2") {
        return Err(ApiError::bad_request(
            "only ListObjectsV2 (list-type=2) and ListMultipartUploads (?uploads) are supported on this path",
        ));
    }
    let who = caller(ext)?;
    let bucket = load_bucket(&c, &who, &bucket_name)?;
    let max_keys = parse_max_keys(q.max_keys.as_deref())?;
    let after = q.continuation_token.as_deref().or(q.start_after.as_deref());
    let (items, truncated) =
        c.list_objects_filtered(&bucket.meta.id, q.prefix.as_deref(), after, max_keys)?;
    let next = truncated
        .then(|| items.last().map(|o| o.spec.key.clone()))
        .flatten();
    let xml = list_bucket_xml(
        &bucket.meta.name,
        q.prefix.as_deref().unwrap_or(""),
        max_keys,
        &items,
        truncated,
        next.as_deref(),
        q.continuation_token.as_deref(),
    );
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

async fn list_uploads_inner(
    c: Client,
    ext: Option<Extension<Caller>>,
    bucket_name: String,
    q: ListQuery,
) -> ApiResult<Response> {
    let who = caller(ext)?;
    let bucket = load_bucket(&c, &who, &bucket_name)?;
    if q.delimiter.as_deref().is_some_and(|d| !d.is_empty()) {
        return Err(multipart::s3_err(
            "InvalidArgument",
            "delimiter is not supported for ListMultipartUploads",
        ));
    }
    let max_uploads = parse_limit(q.max_uploads.as_deref(), "max-uploads", S3_MAX_UPLOADS)?;
    let prefix = q.prefix.as_deref().unwrap_or("");
    let key_marker = q.key_marker.as_deref().unwrap_or("");
    // S3 ignores upload-id-marker unless key-marker is set.
    let upload_id_marker = if key_marker.is_empty() {
        ""
    } else {
        q.upload_id_marker.as_deref().unwrap_or("")
    };
    let (uploads, truncated) = c
        .list_multipart_uploads(
            &bucket.meta.id,
            Some(prefix),
            Some(key_marker),
            Some(upload_id_marker),
            max_uploads,
        )
        .await?;
    let xml = list_uploads_xml(&ListUploadsPage {
        bucket: &bucket.meta.name,
        prefix,
        key_marker,
        upload_id_marker,
        max_uploads,
        uploads: &uploads,
        truncated,
    });
    Ok((
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/xml")],
        xml,
    )
        .into_response())
}

struct ListUploadsPage<'a> {
    bucket: &'a str,
    prefix: &'a str,
    key_marker: &'a str,
    upload_id_marker: &'a str,
    max_uploads: usize,
    uploads: &'a [multipart::UploadSummary],
    truncated: bool,
}

fn list_uploads_xml(page: &ListUploadsPage<'_>) -> String {
    let mut body = String::new();
    body.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>");
    body.push_str("<ListMultipartUploadsResult xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">");
    body.push_str(&format!("<Bucket>{}</Bucket>", xml_escape(page.bucket)));
    body.push_str(&format!(
        "<KeyMarker>{}</KeyMarker>",
        xml_escape(page.key_marker)
    ));
    body.push_str(&format!(
        "<UploadIdMarker>{}</UploadIdMarker>",
        xml_escape(page.upload_id_marker)
    ));
    if page.truncated {
        if let Some(last) = page.uploads.last() {
            body.push_str(&format!(
                "<NextKeyMarker>{}</NextKeyMarker>",
                xml_escape(&last.key)
            ));
            body.push_str(&format!(
                "<NextUploadIdMarker>{}</NextUploadIdMarker>",
                xml_escape(&last.upload_id)
            ));
        }
    }
    body.push_str(&format!("<Prefix>{}</Prefix>", xml_escape(page.prefix)));
    body.push_str(&format!("<MaxUploads>{}</MaxUploads>", page.max_uploads));
    body.push_str(&format!(
        "<IsTruncated>{}</IsTruncated>",
        if page.truncated { "true" } else { "false" }
    ));
    for u in page.uploads {
        body.push_str("<Upload>");
        body.push_str(&format!("<Key>{}</Key>", xml_escape(&u.key)));
        body.push_str(&format!(
            "<UploadId>{}</UploadId>",
            xml_escape(&u.upload_id)
        ));
        body.push_str("<StorageClass>STANDARD</StorageClass>");
        body.push_str(&format!(
            "<Initiated>{}</Initiated>",
            u.initiated.format("%Y-%m-%dT%H:%M:%S%.3fZ")
        ));
        body.push_str("</Upload>");
    }
    body.push_str("</ListMultipartUploadsResult>");
    body
}

fn parse_max_keys(raw: Option<&str>) -> ApiResult<usize> {
    parse_limit(raw, "max-keys", S3_MAX_KEYS)
}

/// `None`/empty → `max`; negative or non-numeric → 400; else clamp to
/// `1..=max`.
fn parse_limit(raw: Option<&str>, name: &str, max: usize) -> ApiResult<usize> {
    match raw {
        None | Some("") => Ok(max),
        Some(s) => {
            let n: i64 = s
                .parse()
                .map_err(|_| ApiError::bad_request(format!("{name} must be a number")))?;
            if n < 0 {
                return Err(ApiError::bad_request(format!("{name} must be >= 0")));
            }
            Ok((n as usize).clamp(1, max))
        }
    }
}

fn apply_object_headers(headers: &mut HeaderMap, obj: &Object) -> ApiResult<()> {
    let ctype = HeaderValue::from_str(&obj.spec.content_type)
        .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream"));
    headers.insert(header::CONTENT_TYPE, ctype);
    if let Ok(len) = HeaderValue::from_str(&obj.status.size_bytes.to_string()) {
        headers.insert(header::CONTENT_LENGTH, len);
    }
    if let Some(etag) = etag_value(obj) {
        headers.insert(header::ETAG, etag);
    }
    let last_mod = obj.meta.updated_at.format("%a, %d %b %Y %H:%M:%S GMT");
    if let Ok(v) = HeaderValue::from_str(&last_mod.to_string()) {
        headers.insert(header::LAST_MODIFIED, v);
    }
    Ok(())
}

fn etag_value(obj: &Object) -> Option<HeaderValue> {
    let hex = if !obj.status.md5.is_empty() {
        &obj.status.md5
    } else if !obj.status.sha256.is_empty() {
        &obj.status.sha256
    } else {
        return None;
    };
    HeaderValue::from_str(&format!("\"{hex}\"")).ok()
}

fn content_length(headers: &HeaderMap) -> ApiResult<Option<u64>> {
    match headers.get(header::CONTENT_LENGTH) {
        None => Ok(None),
        Some(v) => {
            let s = v
                .to_str()
                .map_err(|_| ApiError::bad_request("invalid Content-Length"))?;
            let n: u64 = s
                .parse()
                .map_err(|_| ApiError::bad_request("invalid Content-Length"))?;
            Ok(Some(n))
        }
    }
}

fn caller(ext: Option<Extension<Caller>>) -> ApiResult<Caller> {
    ext.map(|Extension(c)| c)
        .ok_or_else(|| ApiError::unauthorized("no credentials"))
}

fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}

fn list_bucket_xml(
    bucket: &str,
    prefix: &str,
    max_keys: usize,
    items: &[Object],
    truncated: bool,
    next_token: Option<&str>,
    continuation: Option<&str>,
) -> String {
    let mut body = String::new();
    body.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>");
    body.push_str("<ListBucketResult xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">");
    body.push_str(&format!("<Name>{}</Name>", xml_escape(bucket)));
    body.push_str(&format!("<Prefix>{}</Prefix>", xml_escape(prefix)));
    body.push_str(&format!("<MaxKeys>{max_keys}</MaxKeys>"));
    body.push_str(&format!(
        "<IsTruncated>{}</IsTruncated>",
        if truncated { "true" } else { "false" }
    ));
    body.push_str("<KeyCount>");
    body.push_str(&items.len().to_string());
    body.push_str("</KeyCount>");
    if let Some(t) = continuation {
        body.push_str(&format!(
            "<ContinuationToken>{}</ContinuationToken>",
            xml_escape(t)
        ));
    }
    if let Some(t) = next_token {
        body.push_str(&format!(
            "<NextContinuationToken>{}</NextContinuationToken>",
            xml_escape(t)
        ));
    }
    for obj in items {
        let etag = if !obj.status.md5.is_empty() {
            &obj.status.md5
        } else {
            &obj.status.sha256
        };
        body.push_str("<Contents>");
        body.push_str(&format!("<Key>{}</Key>", xml_escape(&obj.spec.key)));
        body.push_str(&format!("<Size>{}</Size>", obj.status.size_bytes));
        body.push_str(&format!("<ETag>&quot;{}&quot;</ETag>", xml_escape(etag)));
        body.push_str(&format!(
            "<LastModified>{}</LastModified>",
            obj.meta.updated_at.format("%Y-%m-%dT%H:%M:%S%.3fZ")
        ));
        body.push_str("<StorageClass>STANDARD</StorageClass>");
        body.push_str("</Contents>");
    }
    body.push_str("</ListBucketResult>");
    body
}

fn s3_error(err: ApiError) -> Response {
    let (status, default_code) = match err.status() {
        StatusCode::NOT_FOUND => (StatusCode::NOT_FOUND, "NoSuchKey"),
        StatusCode::UNAUTHORIZED => (StatusCode::FORBIDDEN, "AccessDenied"),
        StatusCode::FORBIDDEN => (StatusCode::FORBIDDEN, "AccessDenied"),
        StatusCode::BAD_REQUEST => (StatusCode::BAD_REQUEST, "InvalidArgument"),
        StatusCode::CONFLICT => (StatusCode::CONFLICT, "Conflict"),
        other => (other, "InternalError"),
    };
    let code = if let Some(c) = s3_code_of(&err) {
        c
    } else if status == StatusCode::NOT_FOUND
        && (err.message.contains("bkt ") || err.message.contains("bucket"))
    {
        "NoSuchBucket"
    } else {
        default_code
    };
    let xml = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><Error><Code>{}</Code><Message>{}</Message></Error>",
        code,
        xml_escape(&err.message)
    );
    (status, [(header::CONTENT_TYPE, "application/xml")], xml).into_response()
}
