// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::find_object_open::find_object_open;
use super::parse_bool_text::parse_bool_text;
use super::xml_text_opt::xml_text_opt;

/// `<Quiet>` that appears before the first `<Object>`, so an object's own
/// flag is not mistaken for the request-level one.
pub(crate) fn top_level_quiet(body: &str) -> bool {
    let head = match find_object_open(body) {
        Some(idx) => &body[..idx],
        None => body,
    };
    match xml_text_opt(head, "Quiet") {
        Some(v) => parse_bool_text(&v).unwrap_or(false),
        None => false,
    }
}
