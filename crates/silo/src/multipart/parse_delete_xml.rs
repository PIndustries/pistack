// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::ApiResult;

use super::attribute::attribute;
use super::delete_objects_body::DeleteObjectsBody;
use super::delete_request::DeleteRequest;
use super::find_object_open::find_object_open;
use super::normalize_key::normalize_key;
use super::parse_bool_text::parse_bool_text;
use super::s3_err::s3_err;
use super::top_level_quiet::top_level_quiet;
use super::xml_text::xml_text;
use super::xml_text_opt::xml_text_opt;

/// Parse a DeleteObjects body. The tag walk is deliberately shallow — the
/// same shape [`parse_complete_xml`] accepts and nothing else. A body with no
/// `<Object>` is `MalformedXML`, as it is on S3.
pub fn parse_delete_xml(body: &str) -> ApiResult<DeleteObjectsBody> {
    let mut out = DeleteObjectsBody {
        objects: Vec::new(),
        quiet: top_level_quiet(body),
    };
    let mut rest = body;
    while let Some(idx) = find_object_open(rest) {
        let after = &rest[idx..];
        let Some(gt) = after.find('>') else {
            return Err(s3_err("MalformedXML", "unclosed <Object>"));
        };
        let tag = after[..gt].trim();
        let (block, consumed) = if tag.ends_with('/') {
            // `<Object/>` carries its key as an attribute.
            (None, gt + 1)
        } else {
            let Some(close) = after[gt..].find("</Object>") else {
                return Err(s3_err("MalformedXML", "unclosed <Object>"));
            };
            (Some(&after[gt + 1..gt + close]), gt + close + 9)
        };
        let raw_key = match block {
            Some(block) => xml_text(block, "Key")?,
            None => attribute(tag, "Key")
                .ok_or_else(|| s3_err("MalformedXML", "missing <Key> in DeleteObjects"))?
                .to_owned(),
        };
        let raw_quiet = match block {
            Some(block) => xml_text_opt(block, "Quiet"),
            None => attribute(tag, "Quiet").map(str::to_owned),
        };
        out.objects.push(DeleteRequest {
            key: normalize_key(&raw_key),
            quiet: raw_quiet.as_deref().map(parse_bool_text).transpose()?,
        });
        rest = &after[consumed..];
    }
    if out.objects.is_empty() {
        return Err(s3_err("MalformedXML", "no <Object> in DeleteObjects"));
    }
    Ok(out)
}
