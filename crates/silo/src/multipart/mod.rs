// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Multipart upload staging under `{root}/{bucket}/.uploads/{uploadId}/`.
//!
//! Each staging dir holds `meta.json` (bucket, key, content type, initiation
//! time) plus `part-N` / `part-N.md5`. Abort and complete remove the dir;
//! uploads a client never finishes are removed by [`crate::gc`] once they
//! are older than `[modules.silo] multipart_ttl_secs`.
//!
//! Staging is plain files on disk, never store records, so an unfinished
//! upload is not an object: it does not show up in object listings or
//! object sizes. [`crate::staging`] copies it to the other control-plane
//! servers.

mod attribute;
mod client_multipart;
mod completed_part;
mod concat_parts_stream;
mod concat_state;
mod delete_objects_body;
mod delete_request;
mod find_object_open;
mod hex_decode_md5;
mod hex_encode;
mod io_err;
mod list_upload_entry;
mod max_part_number;
mod min_part_bytes;
mod mtime;
mod normalize_etag;
mod normalize_key;
mod parse_bool_text;
mod parse_complete_xml;
mod parse_delete_xml;
mod part_info;
mod part_written;
mod read_staged;
mod s3_code_of;
mod s3_err;
mod scan_staged;
mod staged_upload;
#[cfg(test)]
mod tests;
mod top_level_quiet;
mod upload_meta;
mod upload_summary;
mod validate_upload_id;
mod write_part_atomic;
mod xml_text;
mod xml_text_opt;
mod xml_unescape;
pub use completed_part::CompletedPart;
pub use delete_objects_body::DeleteObjectsBody;
pub use delete_request::DeleteRequest;
pub(crate) use io_err::io_err;
pub use list_upload_entry::ListUploadEntry;
pub use max_part_number::MAX_PART_NUMBER;
pub use min_part_bytes::MIN_PART_BYTES;
pub use parse_complete_xml::parse_complete_xml;
pub use parse_delete_xml::parse_delete_xml;
pub use part_info::PartInfo;
pub use s3_code_of::s3_code_of;
pub use s3_err::s3_err;
pub(crate) use scan_staged::scan_staged;
pub use upload_meta::UploadMeta;
pub use upload_summary::UploadSummary;
pub(crate) use validate_upload_id::validate_upload_id;
