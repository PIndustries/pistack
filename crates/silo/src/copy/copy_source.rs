// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// Parsed `x-amz-copy-source` value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopySource {
    pub bucket: String,
    pub key: String,
    /// Present when the client asked for a version; silo has no object
    /// versioning API for copies yet → callers return `NotImplemented`.
    pub version_id: Option<String>,
}
