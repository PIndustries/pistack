// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::Phase;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct SecurityGroupStatus {
    #[serde(default)]
    pub phase: Phase,
    /// Ports currently members of the group.
    #[serde(default)]
    pub members: u32,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub message: String,
    #[serde(default)]
    pub observed_revision: u64,
}
