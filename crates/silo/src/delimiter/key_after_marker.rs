// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::is_common_prefix_marker::is_common_prefix_marker;

/// Exclusive start-after for a key under optional delimiter grouping.
pub(crate) fn key_after_marker(key: &str, after: &str, delimiter: Option<&str>) -> bool {
    match delimiter {
        Some(d) if is_common_prefix_marker(after, d) => key > after && !key.starts_with(after),
        _ => key > after,
    }
}
