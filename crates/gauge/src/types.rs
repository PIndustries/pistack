// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Alarm resources and the sample point posted to gauge.

use chrono::{DateTime, Utc};
use pistack_types::{kind, Resource, Spec};
use serde::{Deserialize, Serialize};

/// Points kept per series. At a report every fifteen seconds that is a
/// quarter of an hour.
pub const WINDOW_POINTS: usize = 60;
/// No point is kept longer than this.
pub const WINDOW_AGE: chrono::Duration = chrono::Duration::minutes(60);
/// Series one server keeps. A machine with one disk and one interface is
/// about a dozen, so this is room for some four thousand machines.
pub const MAX_SERIES: usize = 50_000;
/// A series with no sample for this long is left out of `/metrics` and of
/// alarms: its machine or host is gone or has stopped reporting.
pub const STALE_AFTER: chrono::Duration = chrono::Duration::minutes(5);
/// How long an alarm's `last_value` may lag while its phase stays the same.
/// Each change of it is a write to the store.
pub const ALARM_REFRESH: chrono::Duration = chrono::Duration::seconds(60);

/// One observation. Not a store resource; samples live in [`crate::SampleRing`].
/// `metric` is a series key: a name, or `name{label="value",...}`.
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
    /// A series, or a name and some labels that select several, such as
    /// `pistack_machine_cpu_usage_cores{machine="mach-..."}` or just
    /// `pistack_host_load1`. The alarm fires while any of them crosses.
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
