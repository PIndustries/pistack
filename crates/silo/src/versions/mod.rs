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

mod after_matches;
mod generation;
mod generations_in;
mod head;
mod is_latest;
mod list_generations;
mod newest_data;
mod next_version;
mod no_such_version;
mod parse_version_id;
mod version_page;
pub(crate) use generation::generation;
pub(crate) use is_latest::is_latest;
pub(crate) use list_generations::list_generations;
pub(crate) use newest_data::newest_data;
pub(crate) use next_version::next_version;
pub(crate) use no_such_version::no_such_version;
pub(crate) use parse_version_id::parse_version_id;
