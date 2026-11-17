// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `is_valid_node_name`: the character set the agent protocol accepts.

use super::max_node_name::MAX_NODE_NAME;

/// 1..=253 ASCII letters, digits, `-`, `.` and `_`. The agent protocol
/// rejects anything else.
pub fn is_valid_node_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_NODE_NAME
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_'))
}
