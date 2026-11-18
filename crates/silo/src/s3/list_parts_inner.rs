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
use super::xml_escape::xml_escape;

pub(crate) async fn list_parts_inner(
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
