// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// Prometheus metric names are `[a-zA-Z_:][a-zA-Z0-9_:]*`. Any other
/// character becomes `_`, and a leading digit gets one in front.
pub fn sanitize_name(name: &str) -> String {
    sanitize(name, true)
}

/// Label names are the same without the colon.
pub(crate) fn sanitize_label(name: &str) -> String {
    sanitize(name, false)
}

fn sanitize(name: &str, colon: bool) -> String {
    let mut out: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || (colon && c == ':') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if out.is_empty() {
        return "_".into();
    }
    if out.as_bytes()[0].is_ascii_digit() {
        out.insert(0, '_');
    }
    out
}
