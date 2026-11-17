// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::*;
use chrono::{DateTime, Utc};

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
