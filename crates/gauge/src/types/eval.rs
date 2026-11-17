// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::alarm_phase::AlarmPhase;
use super::alarm_spec::AlarmSpec;
use super::comparison::Comparison;
use super::sample::Sample;
use super::window::MAX_HOLD;

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
