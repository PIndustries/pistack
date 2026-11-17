// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use serde::{Deserialize, Serialize};

use super::alarm_phase::AlarmPhase;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct AlarmStatus {
    pub phase: AlarmPhase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_value: Option<f64>,
    /// The series `last_value` is from: the one that crosses the furthest,
    /// or comes closest to it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub series: Option<String>,
}

impl Default for AlarmStatus {
    fn default() -> Self {
        Self {
            phase: AlarmPhase::Pending,
            last_value: None,
            series: None,
        }
    }
}
