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

/// Default largest object body accepted by a single PUT (5 GiB).
///
/// Matches the historical S3 single-PUT ceiling. Override per client with
/// [`crate::Client::with_max_object_bytes`] or `[modules.silo]
/// max_object_bytes` (see [`SiloConfig`]).
pub const MAX_OBJECT_BYTES: usize = 5 * 1024 * 1024 * 1024;

/// Default minimum size of every multipart part except the last (5 MiB).
pub const MIN_PART_BYTES: usize = 5 * 1024 * 1024;

/// Module settings under `[modules.silo]` / `PISTACK__MODULES__SILO__*`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct SiloConfig {
    /// Largest accepted object body, in bytes.
    pub max_object_bytes: usize,
}

impl Default for SiloConfig {
    fn default() -> Self {
        Self {
            max_object_bytes: MAX_OBJECT_BYTES,
        }
    }
}

impl SiloConfig {
    /// Reject `max_object_bytes = 0` (unset uses [`MAX_OBJECT_BYTES`]).
    pub fn validate(&self) -> Result<(), pistack_config::ConfigError> {
        if self.max_object_bytes == 0 {
            return Err(pistack_config::ConfigError::Module {
                module: "silo".into(),
                message: "max_object_bytes must be greater than 0".into(),
            });
        }
        Ok(())
    }

    /// Read `[modules.silo]` from a loaded config. Missing section → defaults.
    /// A present section with `max_object_bytes = 0` is rejected.
    pub fn from_config(cfg: &pistack_config::Config) -> Result<Self, pistack_config::ConfigError> {
        let silo: Self = cfg.module("silo")?;
        silo.validate()?;
        Ok(silo)
    }

    /// Load process config (file + `PISTACK__*`) and take `[modules.silo]`.
    ///
    /// No config file → defaults (5 GiB). Invalid silo settings (including
    /// `max_object_bytes = 0`) return an error so assemble can refuse to start.
    pub fn load() -> Result<Self, pistack_config::ConfigError> {
        let cfg = pistack_config::Config::load(None)?;
        Self::from_config(&cfg)
    }
}

/// Human-readable size for error messages (`5 GiB`, `64 MiB`, …).
pub fn format_byte_limit(bytes: usize) -> String {
    const GIB: usize = 1024 * 1024 * 1024;
    const MIB: usize = 1024 * 1024;
    const KIB: usize = 1024;
    if bytes > 0 && bytes.is_multiple_of(GIB) {
        format!("{} GiB", bytes / GIB)
    } else if bytes > 0 && bytes.is_multiple_of(MIB) {
        format!("{} MiB", bytes / MIB)
    } else if bytes > 0 && bytes.is_multiple_of(KIB) {
        format!("{} KiB", bytes / KIB)
    } else {
        format!("{bytes} bytes")
    }
}

/// `object is larger than …` using [`format_byte_limit`].
pub fn object_too_large_message(max: usize) -> String {
    format!("object is larger than {}", format_byte_limit(max))
}

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
    /// Lower-case hex MD5 of the bytes (S3 ETag for single-PUT). Empty on
    /// objects written before this field existed.
    #[serde(default)]
    pub md5: String,
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
            md5: String::new(),
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
    if key == ".uploads" || key.starts_with(".uploads/") {
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
    use super::*;

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
        assert!(validate_key(".uploads/x").is_err());
        assert!(validate_key(r"foo\bar").is_err());
    }

    #[test]
    fn format_limits() {
        assert_eq!(format_byte_limit(MAX_OBJECT_BYTES), "5 GiB");
        assert_eq!(format_byte_limit(64 * 1024), "64 KiB");
        assert_eq!(format_byte_limit(3), "3 bytes");
        assert!(object_too_large_message(1024).contains("1 KiB"));
    }

    #[test]
    fn silo_config_defaults() {
        assert_eq!(SiloConfig::default().max_object_bytes, MAX_OBJECT_BYTES);
        let cfg = pistack_config::Config::from_toml("[modules.silo]\nmax_object_bytes = 1048576\n")
            .unwrap();
        let silo = SiloConfig::from_config(&cfg).unwrap();
        assert_eq!(silo.max_object_bytes, 1024 * 1024);
    }

    #[test]
    fn silo_config_rejects_zero() {
        let cfg =
            pistack_config::Config::from_toml("[modules.silo]\nmax_object_bytes = 0\n").unwrap();
        let err = SiloConfig::from_config(&cfg).unwrap_err();
        assert!(err.to_string().contains("max_object_bytes"), "{err}");
    }
}
