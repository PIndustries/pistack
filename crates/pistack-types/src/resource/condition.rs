// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `Condition`: one Kubernetes-style line of "what is true and why".

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A condition, Kubernetes-style: one line of "what is true and why".
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Condition {
    #[serde(rename = "type")]
    pub kind: String,
    pub status: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub message: String,
    pub last_transition: DateTime<Utc>,
}
