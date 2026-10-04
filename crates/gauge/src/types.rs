// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Alarm resources and the in-memory sample point posted to gauge.

use chrono::{DateTime, Utc};
use pistack_types::{kind, Resource, Spec};
use serde::{Deserialize, Serialize};

/// Cap on the sample ring held by [`crate::Gauge`]. Oldest points are dropped.
pub const MAX_SAMPLES: usize = 10_000;

/// One observation. Not a store resource; the ring lives on [`crate::Gauge`].
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Sample {
    pub metric: String,
    pub value: f64,
    pub timestamp: DateTime<Utc>,
}

/// How an alarm compares the latest sample to [`AlarmSpec::threshold`].
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Comparison {
    Gt,
    Lt,
    Eq,
}

/// `Ok` — latest sample does not cross. `Firing` — it does.
/// `Pending` — no sample for this metric yet.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum AlarmPhase {
    #[default]
    Pending,
    Ok,
    Firing,
}

/// What the user asked the alarm to watch.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct AlarmSpec {
    pub name: String,
    pub metric: String,
    pub comparison: Comparison,
    pub threshold: f64,
    pub for_seconds: u32,
}

impl Spec for AlarmSpec {
    const KIND: &'static str = kind::ALARM;
}

/// Observed state. `last_value` is the sample the phase was decided from.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct AlarmStatus {
    pub phase: AlarmPhase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_value: Option<f64>,
}

impl Default for AlarmStatus {
    fn default() -> Self {
        Self {
            phase: AlarmPhase::Pending,
            last_value: None,
        }
    }
}

pub type Alarm = Resource<AlarmSpec, AlarmStatus>;

/// Latest sample crosses `threshold` under `comparison`.
/// `eq` is exact (`==`); non-finite values never cross.
pub(crate) fn crosses(comparison: Comparison, value: f64, threshold: f64) -> bool {
    if !value.is_finite() || !threshold.is_finite() {
        return false;
    }
    match comparison {
        Comparison::Gt => value > threshold,
        Comparison::Lt => value < threshold,
        Comparison::Eq => value == threshold,
    }
}

pub(crate) fn phase_for(comparison: Comparison, value: f64, threshold: f64) -> AlarmPhase {
    if crosses(comparison, value, threshold) {
        AlarmPhase::Firing
    } else {
        AlarmPhase::Ok
    }
}

/// Prometheus metric names are `[a-zA-Z_:][a-zA-Z0-9_:]*`. Characters outside
/// `[a-zA-Z0-9_:]` become `_`. A leading digit is prefixed with `_`.
pub(crate) fn sanitize_metric(name: &str) -> String {
    let mut out: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == ':' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if out.is_empty() {
        return "_".into();
    }
    if out.as_bytes()[0].is_ascii_digit() {
        out.insert(0, '_');
    }
    out
}
