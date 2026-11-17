// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// `metric` is a series key: a name, or `name{label="value",...}`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Sample {
    pub metric: String,
    pub value: f64,
    pub timestamp: DateTime<Utc>,
}
