// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// One series as it stands now.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Stat {
    pub metric: String,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    pub value: f64,
    pub timestamp: DateTime<Utc>,
    /// For a counter: how fast it rose between the last two samples, per
    /// second. Absent for a gauge and until there are two samples.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rate: Option<f64>,
}
