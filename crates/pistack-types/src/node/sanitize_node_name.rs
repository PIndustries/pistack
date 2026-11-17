// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `sanitize_node_name`: turn a host name into a node name.

use super::max_node_name::MAX_NODE_NAME;

/// Turn a host name into a node name: other characters become `-`, the
/// result is cut to [`MAX_NODE_NAME`]. Empty input and `localhost`, which
/// every machine shares, give `None`.
pub fn sanitize_node_name(raw: &str) -> Option<String> {
    let mapped: String = raw
        .trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_') {
                c
            } else {
                '-'
            }
        })
        .take(MAX_NODE_NAME)
        .collect();
    let name = mapped.trim_matches(|c| c == '-' || c == '.');
    if name.is_empty()
        || name.eq_ignore_ascii_case("localhost")
        || name.eq_ignore_ascii_case("localhost.localdomain")
    {
        None
    } else {
        Some(name.to_owned())
    }
}
