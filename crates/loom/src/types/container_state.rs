// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

// ---- kinds and well-known keys ------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ContainerState {
    Waiting {
        #[serde(default, skip_serializing_if = "String::is_empty")]
        reason: String,
    },
    Running {
        started_at: DateTime<Utc>,
    },
    Terminated {
        exit_code: i32,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        reason: String,
        finished_at: DateTime<Utc>,
    },
}

impl Default for ContainerState {
    fn default() -> Self {
        ContainerState::Waiting {
            reason: "Pending".into(),
        }
    }
}
