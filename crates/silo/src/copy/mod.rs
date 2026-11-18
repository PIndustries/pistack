// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Helpers for S3 UploadPartCopy / CopyObject (`x-amz-copy-source`, range,
//! conditionals), and for the `Range` header of GetObject. CopyObject rejects
//! `x-amz-copy-source-range`; GetObject reads only its own `Range`.

mod check_copy_source_conditions;
mod copy_source;
mod etag_matches;
mod get_range;
mod object_etag;
mod parse_copy_source;
mod parse_copy_source_range;
mod parse_get_range;
mod parse_http_date;
mod percent_decode;
mod satisfiable;
#[cfg(test)]
mod tests;
pub use check_copy_source_conditions::check_copy_source_conditions;
pub use copy_source::CopySource;
pub use get_range::GetRange;
pub use parse_copy_source::parse_copy_source;
pub use parse_copy_source_range::parse_copy_source_range;
pub use parse_get_range::parse_get_range;
pub use satisfiable::satisfiable;
