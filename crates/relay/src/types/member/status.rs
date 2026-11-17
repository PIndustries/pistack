// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The member as the health checker reported it.

use super::operating_status::OperatingStatus;
use pistack_types::Phase;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct MemberStatus {
    #[serde(default)]
    pub phase: Phase,
    #[serde(default)]
    pub operating_status: OperatingStatus,
    /// Consecutive probe results in the current direction, for rise/fall.
    #[serde(default)]
    pub streak: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_check: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub message: String,
    #[serde(default)]
    pub observed_revision: u64,
}
