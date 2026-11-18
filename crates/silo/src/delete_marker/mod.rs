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

mod action;
mod hidden;
mod key_not_there;
mod marker_detail;
mod marker_status;
mod marker_version_of;
mod new_marker;
mod not_a_readable_object;
mod not_empty;
pub(crate) use action::action;
pub(crate) use action::Action;
pub(crate) use hidden::hidden;
pub(crate) use key_not_there::key_not_there;
pub(crate) use marker_version_of::marker_version_of;
pub(crate) use new_marker::new_marker;
pub(crate) use not_a_readable_object::not_a_readable_object;
pub(crate) use not_empty::not_empty;
