// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Delete markers: the generation a plain delete leaves when a bucket keeps
//! generations.
//!
//! On a bucket with versioning on, a delete that names no `versionId` removes
//! no bytes. It appends a generation at the key that is a delete marker — a
//! record with the next version number, `size_bytes` 0, an empty checksum and
//! nothing on disk. The key's bytes are all still there: an older generation
//! reads with `?versionId=`, and removing the marker makes that generation
//! current again. On a bucket without versioning a delete still takes the
//! whole key, as it always did.
//!
//! [`crate::Client::delete_object`] is the one door: the S3 DeleteObject, the
//! S3 DeleteObjects batch and the JSON object delete all reach a key through
//! it, so the bucket's flag is checked in one place for all three.
//!
//! A key already hidden by a marker is taken away whole rather than marked
//! again. A marker holds no bytes, so there is nothing left for a second
//! delete to hide, and a key can only be emptied by removing the generations
//! under it one by one — which is what a versionId on the delete is for.

use crate::types::{Object, ObjectStatus};
use pistack_types::ApiError;

/// What one delete of a key does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Action {
    /// Records, file and blobs of the key all go. A bucket without versioning,
    /// or a versioned key whose current generation is already a marker.
    RemoveKey,
    /// Nothing is removed; a marker becomes the key's current generation.
    AddMarker,
}

/// Which of the two a delete of `key` is, once the key is known to be there.
pub(crate) fn action(bucket: &crate::types::Bucket, current: &Object) -> Action {
    match (bucket.spec.versioning, current.status.delete_marker) {
        // A live generation on a bucket that keeps generations: hide it.
        (true, false) => Action::AddMarker,
        // A bucket that keeps no generations: the key goes whole.
        (false, _) => Action::RemoveKey,
        // A marker is the current record and holds nothing, so there is
        // nothing left to hide: the delete takes the key and everything
        // under the marker.
        (true, true) => Action::RemoveKey,
    }
}

/// The error a caller is owed when the key holds no record at all.
pub(crate) fn key_not_there(key: &str) -> ApiError {
    ApiError::not_found(format!("object '{key}' not found"))
}

/// A delete marker for `key` at `version`: no bytes, no checksum, size 0.
pub(crate) fn new_marker(bucket: &crate::types::Bucket, key: &str, version: u64) -> Object {
    let spec = crate::types::ObjectSpec {
        bucket: bucket.meta.id.clone(),
        key: key.to_owned(),
        content_type: crate::types::content_type_or_default(None),
    };
    let mut obj = Object::new("object", bucket.meta.project.clone(), spec);
    obj.meta.name = obj.meta.id.to_string();
    obj.status = marker_status(version);
    obj
}

/// The status of a marker at `version`.
pub(crate) fn marker_status(version: u64) -> ObjectStatus {
    ObjectStatus {
        sha256: String::new(),
        md5: String::new(),
        size_bytes: 0,
        version,
        latest: true,
        delete_marker: true,
    }
}

/// The `details` key a marker version travels in.
const MARKER_DETAIL: &str = "delete_marker_version";

/// The marker a read of `key` found hiding it. Carried in the error's details
/// so the S3 route can answer `x-amz-delete-marker` and name the marker.
pub(crate) fn hidden(key: &str, marker: &Object) -> ApiError {
    ApiError::not_found(format!("object '{key}' not found"))
        .with_detail(MARKER_DETAIL, marker.status.version.to_string())
}

/// The answer to a read that named a marker as its `versionId`. S3 refuses to
/// send a marker as an object.
pub(crate) fn not_a_readable_object(marker: &Object) -> ApiError {
    crate::multipart::s3_err(
        "MethodNotAllowed",
        format!(
            "object '{}' is a delete marker: version {}",
            marker.spec.key, marker.status.version
        ),
    )
    .with_detail(MARKER_DETAIL, marker.status.version.to_string())
}

/// The marker version an error names, when it names one.
///
/// [`hidden`] and [`not_a_readable_object`] put it there, so the route can
/// answer the two marker headers off the error the read already produced
/// rather than reading the store a second time.
pub(crate) fn marker_version_of(err: &ApiError) -> Option<u64> {
    err.details
        .get(MARKER_DETAIL)
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse::<u64>().ok())
}

/// The answer for a bucket that still holds records when it is being deleted:
/// `left` generations and markers between the caller and an empty bucket.
pub(crate) fn not_empty(left: usize) -> ApiError {
    crate::multipart::s3_err(
        "BucketNotEmpty",
        format!("the bucket still holds {left} generation(s) or delete marker(s)"),
    )
}
