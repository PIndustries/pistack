// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::Condition;
use serde::{Deserialize, Serialize};

// ---- kinds and well-known keys ------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
pub struct AutoscalerStatus {
    #[serde(default)]
    pub desired_replicas: u32,
    #[serde(default)]
    pub current_replicas: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_metric: Option<f64>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub message: String,
    #[serde(default)]
    pub conditions: Vec<Condition>,
}
