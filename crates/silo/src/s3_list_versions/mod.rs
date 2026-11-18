// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! S3 ListObjectVersions: `GET /{bucket}?versions`.
//!
//! Lists every generation the bucket holds: the current one of each key plus
//! every older generation a versioned bucket has kept, and every delete marker
//! a plain delete left behind. A `<Version>` carries `Key`, `VersionId` (the
//! record's generation number as a decimal string), `IsLatest`, `LastModified`,
//! `ETag` and `Size`. A `<DeleteMarker>` carries `Key`, `VersionId`, `IsLatest`
//! and `LastModified` — a marker has no bytes, so no ETag and no size.
//! `IsLatest` is answered off the key's history: the newest generation of a
//! key, whether or not that generation is a marker. Paging follows the S3
//! shape: `prefix`, `max-keys`, and `key-marker` with `version-id-marker` to
//! resume inside one key; the answer names where the next page starts.

mod list_versions;
mod versions_page;
mod versions_xml;
pub(crate) use list_versions::list_versions;
