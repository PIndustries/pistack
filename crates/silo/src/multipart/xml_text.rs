// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::ApiResult;

use super::s3_err::s3_err;

pub(crate) fn xml_text(block: &str, tag: &str) -> ApiResult<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let Some(s) = block.find(&open) else {
        return Err(s3_err(
            "MalformedXML",
            format!("missing <{tag}> in CompleteMultipartUpload"),
        ));
    };
    let after = &block[s + open.len()..];
    let Some(e) = after.find(&close) else {
        return Err(s3_err(
            "MalformedXML",
            format!("unclosed <{tag}> in CompleteMultipartUpload"),
        ));
    };
    Ok(after[..e].trim().to_owned())
}
