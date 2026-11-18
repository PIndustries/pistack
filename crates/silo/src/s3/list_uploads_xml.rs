// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::multipart::{self};

use super::list_uploads_page::ListUploadsPage;
use super::xml_escape::xml_escape;
use super::xml_list_value::xml_list_value;

pub(crate) fn list_uploads_xml(page: &ListUploadsPage<'_>) -> String {
    let enc = page.encode_url;
    let mut body = String::new();
    body.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>");
    body.push_str("<ListMultipartUploadsResult xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">");
    body.push_str(&format!("<Bucket>{}</Bucket>", xml_escape(page.bucket)));
    body.push_str(&format!(
        "<KeyMarker>{}</KeyMarker>",
        xml_list_value(page.key_marker, enc)
    ));
    body.push_str(&format!(
        "<UploadIdMarker>{}</UploadIdMarker>",
        xml_escape(page.upload_id_marker)
    ));
    if page.truncated {
        if let Some(last) = page.entries.last() {
            match last {
                multipart::ListUploadEntry::Upload(u) => {
                    body.push_str(&format!(
                        "<NextKeyMarker>{}</NextKeyMarker>",
                        xml_list_value(&u.key, enc)
                    ));
                    body.push_str(&format!(
                        "<NextUploadIdMarker>{}</NextUploadIdMarker>",
                        xml_escape(&u.upload_id)
                    ));
                }
                multipart::ListUploadEntry::CommonPrefix(cp) => {
                    body.push_str(&format!(
                        "<NextKeyMarker>{}</NextKeyMarker>",
                        xml_list_value(cp, enc)
                    ));
                    body.push_str("<NextUploadIdMarker></NextUploadIdMarker>");
                }
            }
        }
    }
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
    if enc {
        body.push_str("<EncodingType>url</EncodingType>");
    }
    body.push_str(&format!("<MaxUploads>{}</MaxUploads>", page.max_uploads));
    body.push_str(&format!(
        "<IsTruncated>{}</IsTruncated>",
        if page.truncated { "true" } else { "false" }
    ));
    for e in page.entries {
        match e {
            multipart::ListUploadEntry::Upload(u) => {
                body.push_str("<Upload>");
                body.push_str(&format!("<Key>{}</Key>", xml_list_value(&u.key, enc)));
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
            multipart::ListUploadEntry::CommonPrefix(cp) => {
                body.push_str("<CommonPrefixes><Prefix>");
                body.push_str(&xml_list_value(cp, enc));
                body.push_str("</Prefix></CommonPrefixes>");
            }
        }
    }
    body.push_str("</ListMultipartUploadsResult>");
    body
}
