// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! S3-style `delimiter` / `CommonPrefixes` helpers.
//!
//! Shared by ListObjectsV2 and ListMultipartUploads so both APIs roll up
//! keys the same way. Resuming after a common-prefix marker skips every
//! key under that prefix so the next page neither duplicates nor skips it.

mod collapse_with_delimiter;
mod common_prefix_after;
mod delimited;
mod is_common_prefix_marker;
mod key_after_marker;
#[cfg(test)]
mod tests;
mod upload_after_markers;
pub(crate) use collapse_with_delimiter::collapse_with_delimiter;
pub(crate) use common_prefix_after::common_prefix_after;
pub(crate) use delimited::Delimited;
pub(crate) use key_after_marker::key_after_marker;
pub(crate) use upload_after_markers::upload_after_markers;
