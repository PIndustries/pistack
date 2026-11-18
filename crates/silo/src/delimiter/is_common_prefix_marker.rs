// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// True when `after` is a delimiter-rolled common-prefix marker (non-empty
/// and ends with `delimiter`). Resuming after such a marker must skip
/// every key under that prefix so the next page neither duplicates nor
/// skips the CommonPrefix.
pub(crate) fn is_common_prefix_marker(after: &str, delimiter: &str) -> bool {
    !delimiter.is_empty() && !after.is_empty() && after.ends_with(delimiter)
}
