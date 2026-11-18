// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Bucket and object writes.
//!
//! Bytes for the current generation of a key live at
//! `{root}/{bucket-id}/{key}`, where `root` is `$PISTACK_DATA_DIR/silo` in
//! the module. `status.sha256` is the content address of those bytes;
//! `status.md5` is the S3 single-PUT ETag. With bucket versioning, the
//! previous generation's bytes are kept at
//! `{root}/{bucket-id}/.blobs/{sha256}` and the old record stays with
//! `status.latest = false`.
//!
//! Puts stream the request body to a temp file (hashing as they go), then
//! rename into place. Gets can stream the file back without buffering it.
//!
//! A put lands on one server's disk. With [`Client::with_peers`], a read
//! on a server that has the record but not the bytes fetches them from
//! another server first; see [`crate::replica`].

mod blob_file;
mod clean_name;
mod client_type;
mod current_to_read;
mod hex_encode;
mod into_body_read_err;
mod io_err;
mod list_object_entry;
mod map_stream_err;
mod pick_current;
mod remove_file;
mod sha256_hex;
mod tmp_guard;
#[cfg(test)]
mod tmp_guard_tests;
mod write_stream_atomic;
mod written;
pub use client_type::Client;
pub use into_body_read_err::IntoBodyReadErr;
pub use list_object_entry::ListObjectEntry;
pub(crate) use map_stream_err::map_stream_err;
pub(crate) use tmp_guard::TmpGuard;
