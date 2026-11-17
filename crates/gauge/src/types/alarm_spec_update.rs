// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use serde::Deserialize;

use super::alarm_spec::AlarmSpec;
use super::comparison::Comparison;

/// value. The name is not among them: an alarm keeps the one it has.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct AlarmSpecUpdate {
    pub metric: Option<String>,
    pub comparison: Option<Comparison>,
    pub threshold: Option<f64>,
    pub for_seconds: Option<u32>,
}

impl AlarmSpecUpdate {
    /// This spec with the named fields changed.
    pub fn apply(self, mut spec: AlarmSpec) -> AlarmSpec {
        if let Some(metric) = self.metric {
            spec.metric = metric;
        }
        if let Some(comparison) = self.comparison {
            spec.comparison = comparison;
        }
        if let Some(threshold) = self.threshold {
            spec.threshold = threshold;
        }
        if let Some(for_seconds) = self.for_seconds {
            spec.for_seconds = for_seconds;
        }
        spec
    }
}
