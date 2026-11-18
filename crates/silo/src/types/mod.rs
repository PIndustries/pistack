// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `Bucket` and `Object`.
//!
//! Both are `Resource<Spec, Status>`. The bucket id prefix is
//! `pistack_types::kind::BUCKET` (`bkt`). Objects use `Spec::KIND = "obj"`
//! because that prefix is not in `pistack-types` and this crate does not
//! edit it.

mod bucket;
mod bucket_spec;
mod bucket_status;
mod content_type_or_default;
mod default_content_type;
mod default_latest;
mod default_region;
mod default_version;
mod format_byte_limit;
mod kind_object;
mod max_object_bytes;
mod min_part_bytes;
mod multipart_ttl_secs;
mod object;
mod object_spec;
mod object_status;
mod object_too_large_message;
mod reject_xml_unsafe_key;
mod silo_config;
#[cfg(test)]
mod tests;
mod validate_key;
pub use bucket::Bucket;
pub use bucket_spec::BucketSpec;
pub use bucket_status::BucketStatus;
pub use content_type_or_default::content_type_or_default;
pub use default_region::DEFAULT_REGION;
pub use format_byte_limit::format_byte_limit;
pub use kind_object::KIND_OBJECT;
pub use max_object_bytes::MAX_OBJECT_BYTES;
pub use min_part_bytes::MIN_PART_BYTES;
pub use multipart_ttl_secs::MULTIPART_TTL_SECS;
pub use object::Object;
pub use object_spec::ObjectSpec;
pub use object_status::ObjectStatus;
pub use object_too_large_message::object_too_large_message;
pub use reject_xml_unsafe_key::reject_xml_unsafe_key;
pub use silo_config::SiloConfig;
pub use validate_key::validate_key;
