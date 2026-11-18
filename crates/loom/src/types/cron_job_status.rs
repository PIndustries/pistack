// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use chrono::{DateTime, Utc};
use pistack_types::{Condition, Id};
use serde::{Deserialize, Serialize};

// ---- kinds and well-known keys ------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct CronJobStatus {
    #[serde(default)]
    pub active: Vec<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_schedule_time: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_successful_time: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_schedule_time: Option<DateTime<Utc>>,
    #[serde(default)]
    pub conditions: Vec<Condition>,
}
