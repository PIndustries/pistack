// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::*;
use crate::types::{Sample, MAX_SERIES, WINDOW_AGE, WINDOW_POINTS};
use chrono::{DateTime, Utc};

use chrono::Duration;

fn sample(metric: &str, value: f64, at: DateTime<Utc>) -> Sample {
    Sample {
        metric: metric.into(),
        value,
        timestamp: at,
    }
}

#[test]
fn a_series_keeps_its_last_points_only() {
    let ring = SampleRing::memory();
    let now = Utc::now();
    for i in 0..(WINDOW_POINTS + 5) {
        ring.push(sample("m", i as f64, now));
    }
    ring.push(sample("other", 1.0, now));
    let points = ring.list(Some("m"));
    assert_eq!(points.len(), WINDOW_POINTS);
    assert_eq!(points[0].value, 5.0);
    assert_eq!(ring.latest("m").unwrap().value, (WINDOW_POINTS + 4) as f64);
    assert_eq!(ring.list(None).len(), WINDOW_POINTS + 1);
    assert_eq!(ring.series_count(), 2);
}

#[test]
fn a_late_sample_lands_where_its_stamp_belongs_and_alarms_read_the_newest() {
    let ring = SampleRing::memory();
    let now = Utc::now();
    ring.push(sample("m", 1.0, now));
    // A report carrying a reading it took two minutes ago.
    ring.push(sample("m", 2.0, now - Duration::minutes(2)));
    assert_eq!(ring.latest("m").unwrap().value, 1.0);
    assert_eq!(
        ring.list(Some("m"))
            .iter()
            .map(|s| s.value)
            .collect::<Vec<_>>(),
        vec![2.0, 1.0]
    );
}

#[test]
fn old_points_and_empty_series_are_pruned() {
    let ring = SampleRing::memory();
    let now = Utc::now();
    ring.push(sample("old", 1.0, now - WINDOW_AGE - Duration::seconds(1)));
    ring.push(sample("both", 1.0, now - WINDOW_AGE - Duration::seconds(1)));
    ring.push(sample("both", 2.0, now));
    ring.prune(now);
    assert!(ring.latest("old").is_none());
    assert_eq!(ring.list(Some("both")).len(), 1);
    assert_eq!(ring.series_count(), 1);
}

#[test]
fn new_series_are_refused_past_the_cap_and_old_ones_still_take_points() {
    let ring = SampleRing::memory();
    let now = Utc::now();
    ring.push_all((0..MAX_SERIES).map(|i| sample(&format!("m{i}"), 0.0, now)));
    ring.push(sample("one-too-many", 1.0, now));
    ring.push(sample("m0", 7.0, now));
    assert_eq!(ring.series_count(), MAX_SERIES);
    assert!(ring.latest("one-too-many").is_none());
    assert_eq!(ring.latest("m0").unwrap().value, 7.0);
}

#[test]
fn a_reading_gives_the_rate_between_the_last_two_points() {
    let ring = SampleRing::memory();
    let now = Utc::now();
    ring.push(sample("c", 100.0, now - Duration::seconds(10)));
    assert_eq!(ring.readings(|_| true)[0].rate(), None);
    ring.push(sample("c", 150.0, now));
    let reading = &ring.readings(|key| key == "c")[0];
    assert_eq!(reading.value, 150.0);
    assert_eq!(reading.rate(), Some(5.0));
    // A counter that went back to zero has no rate until the next point.
    ring.push(sample("c", 3.0, now + Duration::seconds(10)));
    assert_eq!(ring.readings(|_| true)[0].rate(), None);
}
