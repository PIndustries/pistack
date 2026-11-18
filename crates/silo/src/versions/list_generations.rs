// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::{Client, ListObjectEntry};
use pistack_types::{ApiResult, Id};

use super::after_matches::after_matches;
use super::generations_in::generations_in;
use super::version_page::VersionPage;

/// Generations of each key in the bucket, newest generation first within a
/// key, optionally filtered by `prefix` and resumed after a
/// `(key-marker, version-id-marker)` pair (exclusive), capped at `limit`.
///
/// A delete marker is a generation like any other and is listed among them, so
/// the caller can see why a key is missing and name the marker on a delete.
///
/// `limit + 1` rows are collected so `truncated` can be answered exactly;
/// the page itself holds `limit`.
pub(crate) fn list_generations(
    c: &Client,
    bucket_id: &Id,
    prefix: Option<&str>,
    delimiter: Option<&str>,
    after: Option<(&str, Option<&str>)>,
    limit: usize,
) -> ApiResult<VersionPage> {
    let prefix = prefix.unwrap_or("");
    let delimiter = delimiter.filter(|d| !d.is_empty());
    let wanted = limit.saturating_add(1);
    let mut entries: Vec<ListObjectEntry> = Vec::new();
    let mut rolled: Option<String> = None;
    for obj in generations_in(c, bucket_id)? {
        let key = obj.spec.key.as_str();
        if !prefix.is_empty() && !key.starts_with(prefix) {
            continue;
        }
        if !after_matches(key, obj.status.version, after) {
            continue;
        }
        match delimiter.and_then(|d| crate::delimiter::common_prefix_after(key, prefix, d)) {
            // One row per rolled-up prefix, and the marker resumes past all
            // of the keys filed under it.
            Some(cp) => {
                if rolled.as_deref() == Some(cp.as_str()) {
                    continue;
                }
                rolled = Some(cp.clone());
                entries.push(ListObjectEntry::CommonPrefix(cp));
            }
            None => {
                rolled = None;
                entries.push(ListObjectEntry::Object(Box::new(obj)));
            }
        }
        if entries.len() >= wanted {
            break;
        }
    }
    let truncated = entries.len() > limit;
    entries.truncate(limit);
    Ok((entries, truncated))
}
