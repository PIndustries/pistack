// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use serde::{Deserialize, Serialize};

use super::default_latest::default_latest;
use super::default_version::default_version;

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
    /// True for the generation a plain delete left in place of the bytes: no
    /// bytes on disk, size 0, and the key is hidden until it is removed.
    #[serde(default)]
    pub delete_marker: bool,
}

impl Default for ObjectStatus {
    fn default() -> Self {
        Self {
            sha256: String::new(),
            md5: String::new(),
            size_bytes: 0,
            version: default_version(),
            latest: default_latest(),
            delete_marker: false,
        }
    }
}
