// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::list_bucket_page::ListBucketPage;
use super::xml_escape::xml_escape;
use super::xml_list_value::xml_list_value;

pub(crate) fn list_bucket_xml(page: &ListBucketPage<'_>) -> String {
    let enc = page.encode_url;
    let mut body = String::new();
    body.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>");
    body.push_str("<ListBucketResult xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">");
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
    body.push_str(&format!("<MaxKeys>{}</MaxKeys>", page.max_keys));
    body.push_str(&format!(
        "<IsTruncated>{}</IsTruncated>",
        if page.truncated { "true" } else { "false" }
    ));
    body.push_str("<KeyCount>");
    body.push_str(&page.entries.len().to_string());
    body.push_str("</KeyCount>");
    if enc {
        body.push_str("<EncodingType>url</EncodingType>");
    }
    if let Some(t) = page.continuation {
        // Echo the client-supplied token as sent (already opaque base64).
        body.push_str(&format!(
            "<ContinuationToken>{}</ContinuationToken>",
            xml_escape(t)
        ));
    }
    if !page.start_after.is_empty() && page.continuation.is_none() {
        body.push_str(&format!(
            "<StartAfter>{}</StartAfter>",
            xml_list_value(page.start_after, enc)
        ));
    }
    if let Some(t) = page.next_token {
        // Opaque URL-safe base64 of the resume key (already encoded).
        body.push_str(&format!(
            "<NextContinuationToken>{}</NextContinuationToken>",
            xml_escape(t)
        ));
    }
    for e in page.entries {
        match e {
            crate::client::ListObjectEntry::Object(obj) => {
                let obj = obj.as_ref();
                let etag = if !obj.status.md5.is_empty() {
                    &obj.status.md5
                } else {
                    &obj.status.sha256
                };
                body.push_str("<Contents>");
                body.push_str(&format!(
                    "<Key>{}</Key>",
                    xml_list_value(&obj.spec.key, enc)
                ));
                body.push_str(&format!("<Size>{}</Size>", obj.status.size_bytes));
                body.push_str(&format!("<ETag>&quot;{}&quot;</ETag>", xml_escape(etag)));
                body.push_str(&format!(
                    "<LastModified>{}</LastModified>",
                    obj.meta.updated_at.format("%Y-%m-%dT%H:%M:%S%.3fZ")
                ));
                body.push_str("<StorageClass>STANDARD</StorageClass>");
                body.push_str("</Contents>");
            }
            crate::client::ListObjectEntry::CommonPrefix(cp) => {
                body.push_str("<CommonPrefixes><Prefix>");
                body.push_str(&xml_list_value(cp, enc));
                body.push_str("</Prefix></CommonPrefixes>");
            }
        }
    }
    body.push_str("</ListBucketResult>");
    body
}
