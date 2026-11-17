// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Object generations: the record of one `versionId`, and the whole history
//! of a bucket for ListObjectVersions.
//!
//! The bytes at a key path are always the current generation. An older
//! generation kept because the bucket has versioning on is a file under
//! `.blobs`, named by its checksum, and its record answers with
//! `status.latest = false`. A `versionId` on the wire is a record's
//! `status.version` as a decimal string — there is no version table and no
//! separate version identity.

use crate::client::{Client, ListObjectEntry};
use crate::types::Object;
use pistack_types::{ApiError, ApiResult, Id};

/// A `versionId` as sent on the wire: a generation number in decimal.
pub(crate) fn parse_version_id(raw: &str) -> ApiResult<u64> {
    raw.trim().parse::<u64>().map_err(|_| no_such_version(raw))
}

/// The S3 `NoSuchVersion` answer for a generation that is not there.
pub(crate) fn no_such_version(raw: &str) -> ApiError {
    crate::multipart::s3_err(
        "NoSuchVersion",
        format!("The version ID specified in the request does not exist: {raw}"),
    )
}

/// The record standing at `key`'s newest generation, marker or not, or `None`
/// when the key holds no record.
///
/// The current generation of a key is its newest one, and a plain delete on a
/// versioned bucket leaves a delete marker as that generation, so this is the
/// record an S3 answer is taken from — unlike the record `put_here` replaces,
/// which is the newest one that holds bytes.
pub(crate) fn head(all: &[Object], key: &str) -> Option<Object> {
    all.iter()
        .filter(|o| o.spec.key == key)
        .max_by_key(|o| (o.status.version, o.meta.revision))
        .cloned()
}

/// The newest generation of `key` that holds bytes, ignoring delete markers —
/// the record a put builds on, and the one the key path's file belongs to.
pub(crate) fn newest_data(all: &[Object], key: &str) -> Option<Object> {
    all.iter()
        .filter(|o| o.spec.key == key && !o.status.delete_marker)
        .max_by_key(|o| (o.status.version, o.meta.revision))
        .cloned()
}

/// The next generation number at `key`, counting markers: a key whose three
/// generations ended in a plain delete is written at 4, not 3.
pub(crate) fn next_version(all: &[Object], key: &str) -> u64 {
    head(all, key)
        .map(|o| o.status.version.saturating_add(1))
        .unwrap_or(1)
}

/// The record of `key` at generation `version` among `all`, or
/// `NoSuchVersion`.
pub(crate) fn generation(all: &[Object], key: &str, version: u64) -> ApiResult<Object> {
    let mut found: Option<Object> = None;
    for obj in all {
        if obj.spec.key != key || obj.status.version != version {
            continue;
        }
        // Two records claiming one generation means a writer raced; the newer
        // row is the one still standing.
        if found
            .as_ref()
            .is_none_or(|cur| obj.meta.revision >= cur.meta.revision)
        {
            found = Some(obj.clone());
        }
    }
    found.ok_or_else(|| no_such_version(&version.to_string()))
}

/// True when `obj` is the newest generation of its key — the row ListObjectVersions
/// answers `IsLatest` for. Taken from the history rather than `status.latest`,
/// because a marker left on top of a key makes that key's newest generation
/// while the record under it still carries the flag.
pub(crate) fn is_latest(all: &[Object], obj: &Object) -> bool {
    head(all, &obj.spec.key).is_some_and(|cur| cur.status.version == obj.status.version)
}

/// One row of a ListObjectVersions page: a kept generation, a prefix rolled up
/// by the delimiter, or a delete marker.
pub type VersionPage = (Vec<ListObjectEntry>, bool);

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

/// Every generation of every key in the bucket, key order first and newest
/// generation first within a key — the order S3 lists versions in. The current
/// generation of a key is its newest one, marker included, so `latest` is
/// answered off the version rather than the flag a put leaves behind.
fn generations_in(c: &Client, bucket_id: &Id) -> ApiResult<Vec<Object>> {
    let mut all = c.list_all_objects(bucket_id)?;
    all.sort_by(|a, b| {
        a.spec
            .key
            .cmp(&b.spec.key)
            .then(b.status.version.cmp(&a.status.version))
    });
    Ok(all)
}

/// `true` when `(key, version)` comes after the paging marker.
///
/// Generations are listed newest first, so the marker's own key is skipped
/// whole once a newer generation of it has been emitted: the version-id
/// marker narrows only the key marker's own key, as S3 defines it. A
/// version-id marker that is not a generation number stops after the whole
/// key.
fn after_matches(key: &str, version: u64, after: Option<(&str, Option<&str>)>) -> bool {
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
