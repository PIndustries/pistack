// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// `true` when `(key, version)` comes after the paging marker.
///
/// Generations are listed newest first, so the marker's own key is skipped
/// whole once a newer generation of it has been emitted: the version-id
/// marker narrows only the key marker's own key, as S3 defines it. A
/// version-id marker that is not a generation number stops after the whole
/// key.
pub(crate) fn after_matches(key: &str, version: u64, after: Option<(&str, Option<&str>)>) -> bool {
    let Some((marker_key, marker_version)) = after else {
        return true;
    };
    if key != marker_key {
        return key > marker_key;
    }
    match marker_version.and_then(|v| v.parse::<u64>().ok()) {
        Some(marker) => version < marker,
        None => false,
    }
}
