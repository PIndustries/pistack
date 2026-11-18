// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::ListObjectEntry;
use crate::s3::{xml_escape, xml_list_value};

use super::versions_page::VersionsPage;

pub(crate) fn versions_xml(page: &VersionsPage<'_>) -> String {
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
                let latest = crate::versions::is_latest(&page.all, obj);
                // A marker leads with <DeleteMarker> and has no ETag or Size:
                // it holds no bytes, and there is nothing to name by checksum.
                let tag = if obj.status.delete_marker {
                    "DeleteMarker"
                } else {
                    "Version"
                };
                body.push_str(&format!("<{tag}>"));
                body.push_str(&format!(
                    "<Key>{}</Key>",
                    xml_list_value(&obj.spec.key, enc)
                ));
                body.push_str(&format!("<VersionId>{}</VersionId>", obj.status.version));
                body.push_str(&format!(
                    "<IsLatest>{}</IsLatest>",
                    if latest { "true" } else { "false" }
                ));
                body.push_str(&format!(
                    "<LastModified>{}</LastModified>",
                    obj.meta.updated_at.format("%Y-%m-%dT%H:%M:%S%.3fZ")
                ));
                if tag == "Version" {
                    let etag = if !obj.status.md5.is_empty() {
                        &obj.status.md5
                    } else {
                        &obj.status.sha256
                    };
                    body.push_str(&format!("<ETag>&quot;{}&quot;</ETag>", xml_escape(etag)));
                    body.push_str(&format!("<Size>{}</Size>", obj.status.size_bytes));
                    body.push_str("<StorageClass>STANDARD</StorageClass>");
                }
                body.push_str(&format!("</{tag}>"));
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
