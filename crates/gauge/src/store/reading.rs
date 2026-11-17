// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use chrono::{DateTime, Utc};

/// The latest point of one series, and the one before it when there is one.
#[derive(Clone, Debug, PartialEq)]
pub struct Reading {
    /// The series key, as [`crate::Series::key`] writes it.
    pub metric: String,
    pub value: f64,
    pub timestamp: DateTime<Utc>,
    pub previous: Option<(DateTime<Utc>, f64)>,
}

impl Reading {
    /// How fast the value rose between the last two points, per second.
    /// `None` with one point, or when the value went down (a counter that
    /// started again).
    pub fn rate(&self) -> Option<f64> {
        let (at, value) = self.previous?;
        let secs = (self.timestamp - at).num_milliseconds() as f64 / 1000.0;
        (secs > 0.0 && self.value >= value).then(|| (self.value - value) / secs)
    }
}
