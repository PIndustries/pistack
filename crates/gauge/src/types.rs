// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Alarm resources and the sample point posted to gauge.

use chrono::{DateTime, TimeDelta, Utc};
use pistack_types::{kind, Resource, Spec};
use serde::{Deserialize, Serialize};

/// Points kept per series. At a report every fifteen seconds that is four hours of
/// history, which is the longest hold an alarm can be asked to wait out: a hold is
/// measured on the stamps of the points a series keeps, so nothing beyond this window
/// can ever be waited for. See [`MAX_HOLD`].
pub const WINDOW_POINTS: usize = 240;
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
/// The longest hold an alarm may ask for: a `WINDOW_POINTS` run of samples at the
/// [`WINDOW_AGE`] limit. An alarm asking for more could never fire, so it is refused
/// where it is made rather than left pending forever.
pub const MAX_HOLD: TimeDelta = WINDOW_AGE;

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
    /// How long the value must keep crossing before the alarm fires, in seconds. `0`,
    /// and no `for_seconds` at all, fires on the first sample that crosses. The wait is
    /// measured on the stamps of the samples themselves, so it runs in the clock of the
    /// series and not the clock of the server deciding it. Longer than [`MAX_HOLD`] is
    /// refused: the window a series keeps could never cover it.
    #[serde(default)]
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

/// How long the newest run of crossing points spans, on the stamps of the points.
///
/// `points` is the window of one series, oldest stamp first. The run is the crossing
/// points back from the newest, as far as the window reaches, and its span is the newest
/// stamp minus the oldest of them. A point that does not cross ends the run, and the
/// crossings after it begin a new one; an empty window or a newest point that does not
/// cross spans nothing.
///
/// Nothing here reads a clock, so a run is as long as the series says it is however late
/// its points arrived, and a run that began before the window — the value was crossing
/// for an hour, of which the series kept four — is as long as the points held make it.
pub(crate) fn held_for(spec: &AlarmSpec, points: &[Sample]) -> chrono::Duration {
    let Some(newest) = points.last() else {
        return chrono::Duration::zero();
    };
    if !crosses(spec.comparison, newest.value, spec.threshold) {
        return chrono::Duration::zero();
    }
    let run = points
        .iter()
        .rev()
        .take_while(|p| crosses(spec.comparison, p.value, spec.threshold))
        .count();
    newest.timestamp - points[points.len() - run].timestamp
}

/// The phase an alarm is in, decided from the points of one series and nothing else.
///
/// An alarm asking for no hold is decided from the newest point alone: crossing is
/// `Firing`. One asking for a hold fires once the newest run of crossing points spans at
/// least `for_seconds`, and is held for while it waits.
pub(crate) fn decide_on(spec: &AlarmSpec, points: &[Sample]) -> AlarmPhase {
    let Some(newest) = points.last() else {
        return AlarmPhase::Pending;
    };
    if !crosses(spec.comparison, newest.value, spec.threshold) {
        return AlarmPhase::Ok;
    }
    if spec.for_seconds == 0
        || held_for(spec, points) >= chrono::Duration::seconds(spec.for_seconds as i64)
    {
        AlarmPhase::Firing
    } else {
        AlarmPhase::Ok
    }
}

/// Is a hold this long something the window can ever decide.
pub(crate) fn hold_coverable(for_seconds: u32) -> bool {
    chrono::Duration::seconds(for_seconds as i64) <= MAX_HOLD
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hold_up_to_the_window_is_coverable_and_one_more_is_not() {
        assert!(hold_coverable(0));
        assert!(hold_coverable(MAX_HOLD.num_seconds() as u32));
        assert!(!hold_coverable(MAX_HOLD.num_seconds() as u32 + 1));
    }

    #[test]
    fn the_window_reaches_an_hour_back_and_no_hold_asks_longer() {
        // The window is 240 points and an hour of age, so what it can ever span is an
        // hour: enough for the minute-long holds anyone writes and the quarter-hour ones a
        // host runs, and the ceiling the API refuses above.
        assert_eq!(MAX_HOLD, WINDOW_AGE);
        assert!(hold_coverable(600));
        assert!(hold_coverable(900));
        assert!(hold_coverable(3_600));
    }

    fn points(step: i64, values: &[f64]) -> Vec<Sample> {
        let start = DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        values
            .iter()
            .enumerate()
            .map(|(i, value)| Sample {
                metric: "m".into(),
                value: *value,
                timestamp: start + chrono::Duration::seconds(step * i as i64),
            })
            .collect()
    }

    fn spec(for_seconds: u32) -> AlarmSpec {
        AlarmSpec {
            name: "a".into(),
            metric: "m".into(),
            comparison: Comparison::Gt,
            threshold: 50.0,
            for_seconds,
        }
    }

    #[test]
    fn a_hold_fires_when_the_run_of_crossing_points_spans_it() {
        let run = points(15, &[80.0, 80.0, 80.0, 80.0, 80.0]);
        assert_eq!(decide_on(&spec(60), &run[..4]), AlarmPhase::Ok);
        assert_eq!(decide_on(&spec(60), &run), AlarmPhase::Firing);
    }

    #[test]
    fn a_hold_is_measured_on_the_stamps_and_not_the_number_of_points() {
        let irregular = points(1, &[80.0, 80.0, 80.0]);
        let spread = vec![
            Sample {
                timestamp: irregular[0].timestamp,
                ..irregular[0].clone()
            },
            Sample {
                timestamp: irregular[1].timestamp + chrono::Duration::seconds(59),
                ..irregular[1].clone()
            },
            Sample {
                timestamp: irregular[2].timestamp + chrono::Duration::seconds(60),
                ..irregular[2].clone()
            },
        ];
        assert_eq!(decide_on(&spec(60), &irregular), AlarmPhase::Ok);
        assert_eq!(decide_on(&spec(60), &spread), AlarmPhase::Firing);
    }

    #[test]
    fn a_point_that_does_not_cross_ends_the_run() {
        let run = points(60, &[80.0, 80.0, 10.0, 80.0, 80.0, 80.0]);
        assert_eq!(decide_on(&spec(60), &run), AlarmPhase::Firing);
        assert_eq!(decide_on(&spec(180), &run), AlarmPhase::Ok);
    }

    #[test]
    fn with_no_hold_the_newest_point_decides_alone() {
        assert_eq!(
            decide_on(&spec(0), &points(15, &[80.0])),
            AlarmPhase::Firing
        );
        assert_eq!(decide_on(&spec(0), &points(15, &[10.0])), AlarmPhase::Ok);
        assert_eq!(decide_on(&spec(30), &[]), AlarmPhase::Pending);
    }
}
