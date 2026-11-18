// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// Undo the XML entities a client puts in a key. A `%20` stays as written: a
/// DeleteObjects body carries a key, not a URL.
pub(crate) fn xml_unescape(s: &str) -> String {
    if !s.contains('&') {
        return s.to_owned();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        let (text, used) = match tail.find(';') {
            Some(end) => match &tail[..=end] {
                "&amp;" => ("&", end + 1),
                "&lt;" => ("<", end + 1),
                "&gt;" => (">", end + 1),
                "&quot;" => ("\"", end + 1),
                "&apos;" => ("'", end + 1),
                _ => ("&", 1),
            },
            None => ("&", 1),
        };
        out.push_str(text);
        rest = &tail[used..];
    }
    out.push_str(rest);
    out
}
