// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// After `prefix`, if `delimiter` appears in the remainder, return the
/// rolled-up common prefix (including the delimiter).
///
/// If a key equals the common prefix string (e.g. `a/` with delimiter `/`),
/// S3 would group it into that CommonPrefix; silo currently rejects
/// trailing-slash object keys, so this only matters for the string helper.
pub(crate) fn common_prefix_after(key: &str, prefix: &str, delimiter: &str) -> Option<String> {
    if delimiter.is_empty() || !key.starts_with(prefix) {
        return None;
    }
    let rest = &key[prefix.len()..];
    rest.find(delimiter).map(|i| {
        let end = i + delimiter.len();
        format!("{}{}", prefix, &rest[..end])
    })
}
