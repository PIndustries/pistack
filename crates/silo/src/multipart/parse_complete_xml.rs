// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::ApiResult;

use super::completed_part::CompletedPart;
use super::s3_err::s3_err;
use super::xml_text::xml_text;

/// Parse CompleteMultipartUpload XML into ordered parts.
pub fn parse_complete_xml(body: &str) -> ApiResult<Vec<CompletedPart>> {
    // Minimal tag walk — avoids a XML crate dependency for this one shape.
    let mut parts = Vec::new();
    let mut rest = body;
    while let Some(start) = rest.find("<Part>") {
        let after = &rest[start + 6..];
        let Some(end) = after.find("</Part>") else {
            return Err(s3_err("MalformedXML", "unclosed <Part>"));
        };
        let block = &after[..end];
        let number = xml_text(block, "PartNumber")?
            .parse::<u32>()
            .map_err(|_| s3_err("MalformedXML", "PartNumber must be a number"))?;
        let etag = xml_text(block, "ETag")?;
        parts.push(CompletedPart {
            part_number: number,
            etag,
        });
        rest = &after[end + 7..];
    }
    Ok(parts)
}
