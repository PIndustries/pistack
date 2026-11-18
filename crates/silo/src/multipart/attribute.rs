// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// Value of `name="…"` inside a start tag.
pub(crate) fn attribute<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let open = format!("{name}=\"");
    let s = tag.find(&open)? + open.len();
    let rest = &tag[s..];
    let e = rest.find('"')?;
    Some(&rest[..e])
}
