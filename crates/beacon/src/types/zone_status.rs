// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::Phase;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ZoneStatus {
    #[serde(default)]
    pub phase: Phase,
    /// SOA serial. Bumped by the reconciler each time the zone's content
    /// (spec or any record set) changes. Zero until the first pass.
    #[serde(default)]
    pub serial: u32,
    /// Number of record sets in the zone at the last pass.
    #[serde(default)]
    pub records: u32,
    /// Fingerprint of the content the serial was issued for, so a repeated
    /// pass over unchanged data does not bump the serial again.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub digest: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub message: String,
}
