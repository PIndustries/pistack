// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `Bucket` and `Object`.
//!
//! Both are `Resource<Spec, Status>`. The bucket id prefix is
//! `pistack_types::kind::BUCKET` (`bkt`). Objects use `Spec::KIND = "obj"`
//! because that prefix is not in `pistack-types` and this crate does not
//! edit it.

use pistack_types::{kind, ApiError, ApiResult, Phase, Resource, Spec};
use serde::{Deserialize, Serialize};

/// Id prefix for objects (`obj-<uuid>`).
pub const KIND_OBJECT: &str = "obj";

/// Largest object body accepted by `PUT` (1 GiB).
pub const MAX_OBJECT_BYTES: usize = 1024 * 1024 * 1024;

// ---- Bucket ------------------------------------------------------------------

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct BucketSpec {
    /// When true, a put keeps the previous generation's bytes and record.
    /// The key path always serves the latest bytes either way.
    #[serde(default)]
    pub versioning: bool,
}

impl Spec for BucketSpec {
    const KIND: &'static str = kind::BUCKET;
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct BucketStatus {
    #[serde(default)]
    pub phase: Phase,
}

pub type Bucket = Resource<BucketSpec, BucketStatus>;

// ---- Object ------------------------------------------------------------------

fn default_content_type() -> String {
    "application/octet-stream".to_owned()
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ObjectSpec {
    pub bucket: pistack_types::Id,
    pub key: String,
    #[serde(default = "default_content_type")]
    pub content_type: String,
}

impl Spec for ObjectSpec {
    const KIND: &'static str = KIND_OBJECT;
}

fn default_version() -> u64 {
    1
}

fn default_latest() -> bool {
    true
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ObjectStatus {
    /// Lower-case hex SHA-256 of the bytes at `{bucket}/{key}`.
    #[serde(default)]
    pub sha256: String,
    #[serde(default)]
    pub size_bytes: u64,
    /// Generation at this key. Starts at 1; each put increments it.
    #[serde(default = "default_version")]
    pub version: u64,
    /// False for a generation kept only because the bucket has versioning on.
    #[serde(default = "default_latest")]
    pub latest: bool,
}

impl Default for ObjectStatus {
    fn default() -> Self {
        Self {
            sha256: String::new(),
            size_bytes: 0,
            version: default_version(),
            latest: default_latest(),
        }
    }
}

pub type Object = Resource<ObjectSpec, ObjectStatus>;

/// Reject empty keys, absolute paths, and any `.` / `..` component.
///
/// Object bytes are joined onto the bucket directory, so a key that climbs
/// out of that directory would write outside the data dir.
pub fn validate_key(key: &str) -> ApiResult<()> {
    if key.is_empty() {
        return Err(ApiError::bad_request("object key is empty"));
    }
    if key.contains('\0') {
        return Err(ApiError::bad_request("object key contains NUL"));
    }
    if key.contains('\\') {
        return Err(ApiError::bad_request("object key must not contain '\\'"));
    }
    if key.starts_with('/') {
        return Err(ApiError::bad_request("object key must not be absolute"));
    }
    let bytes = key.as_bytes();
    if bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic() {
        return Err(ApiError::bad_request("object key must not be absolute"));
    }
    if key == ".blobs" || key.starts_with(".blobs/") {
        return Err(ApiError::bad_request("object key is reserved"));
    }
    for seg in key.split('/') {
        if seg.is_empty() || seg == "." || seg == ".." {
            return Err(ApiError::bad_request(
                "object key must be a relative path without '.' or '..'",
            ));
        }
    }
    Ok(())
}

pub fn content_type_or_default(value: Option<String>) -> String {
    match value {
        Some(v) if !v.trim().is_empty() => v,
        _ => default_content_type(),
    }
}

#[cfg(test)]
mod tests {
    use super::validate_key;

    #[test]
    fn key_rules() {
        assert!(validate_key("hello.txt").is_ok());
        assert!(validate_key("dir/nested.txt").is_ok());
        assert!(validate_key("").is_err());
        assert!(validate_key("..").is_err());
        assert!(validate_key("../x").is_err());
        assert!(validate_key("a/../../b").is_err());
        assert!(validate_key("/etc/passwd").is_err());
        assert!(validate_key("a//b").is_err());
        assert!(validate_key(".blobs/abcd").is_err());
        assert!(validate_key(r"foo\bar").is_err());
    }
}
