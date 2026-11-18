// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::Phase;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct RecordSetStatus {
    #[serde(default)]
    pub phase: Phase,
    /// Zone serial the set was first served under.
    #[serde(default)]
    pub serial: u32,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub message: String,
}
