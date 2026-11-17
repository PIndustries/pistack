// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::Sample;
use chrono::{DateTime, Utc};

/// One series an alarm's metric selects, with the points the window holds for it.
pub(crate) struct Choice {
    pub(crate) series: String,
    /// The series' points, oldest stamp first. The newest of them is the point an alarm is
    /// decided from, read from the window rather than trusted from a reading.
    pub(crate) points: Vec<Sample>,
    /// The newest point, and whether it is no older than [`STALE_AFTER`]. Freshness is the
    /// wall clock's judgement and only an alarm with no hold takes it: a series that
    /// stopped reporting keeps the phase its points last decided.
    pub(crate) newest: (DateTime<Utc>, f64),
    pub(crate) fresh: bool,
}

impl Choice {
    pub(crate) fn value(&self) -> f64 {
        self.newest.1
    }
}
