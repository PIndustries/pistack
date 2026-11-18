// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use chrono::{DateTime, Utc};
use pistack_types::Condition;
use serde::{Deserialize, Serialize};

// ---- kinds and well-known keys ------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct JobStatus {
    #[serde(default)]
    pub active: u32,
    #[serde(default)]
    pub succeeded: u32,
    #[serde(default)]
    pub failed: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_time: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion_time: Option<DateTime<Utc>>,
    /// Earliest time a replacement pod may be created after a failure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_retry: Option<DateTime<Utc>>,
    #[serde(default)]
    pub conditions: Vec<Condition>,
}

impl JobStatus {
    pub fn has_condition(&self, kind: &str) -> bool {
        self.conditions.iter().any(|c| c.kind == kind && c.status)
    }
    pub fn is_finished(&self) -> bool {
        self.has_condition("Complete") || self.has_condition("Failed")
    }
}
