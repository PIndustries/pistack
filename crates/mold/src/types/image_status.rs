// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::{Condition, Phase};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ImageStatus {
    /// `Pending`: metadata only, awaiting data. `Creating`: upload in
    /// progress. `Ready`: data stored and inspected. `Error`: rejected.
    #[serde(default)]
    pub phase: Phase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    /// `ImageStore` reference, e.g. `sha256:<hex>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backing_ref: Option<String>,
    /// Absolute blob path on this server. Set only for a local store, so a
    /// machine can boot the file without treating `backing_ref` as a path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_path: Option<String>,
    /// Size of the disk the image expands to (qcow2 virtual size). `None`
    /// when unknown (no `qemu-img` on the server).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub virtual_size_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conditions: Vec<Condition>,
}
