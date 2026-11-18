// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

pub(crate) fn xml_text_opt(block: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let s = block.find(&open)?;
    let after = &block[s + open.len()..];
    let e = after.find(&close)?;
    Some(after[..e].trim().to_owned())
}
