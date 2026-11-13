// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The samples a server holds: a short window per series, in memory.
//!
//! This is not a time-series database. It keeps enough for the API, for
//! `pi machine stats` and for alarms: the last [`WINDOW_POINTS`] points of
//! each series, none older than [`WINDOW_AGE`], for at most [`MAX_SERIES`]
//! series. Nothing is written to disk and nothing is replicated; a server
//! that restarts or takes over as leader starts with an empty window.
//! History is what Prometheus scrapes from `/metrics`.

use crate::types::{Sample, MAX_SERIES, WINDOW_AGE, WINDOW_POINTS};
use chrono::{DateTime, Utc};
use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard};

#[derive(Clone, Copy, Debug, PartialEq)]
struct Point {
    at: DateTime<Utc>,
    value: f64,
}

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

/// Per-series windows of samples. The name is from when this was one ring.
#[derive(Debug, Default)]
pub struct SampleRing {
    series: Mutex<BTreeMap<String, VecDeque<Point>>>,
    full: AtomicBool,
    /// When old points were last dropped.
    pruned: Mutex<Option<DateTime<Utc>>>,
}

/// How often a push also drops what has aged out of the window.
const PRUNE_EVERY: chrono::Duration = chrono::Duration::seconds(60);

impl SampleRing {
    pub fn memory() -> Self {
        Self::default()
    }

    fn lock(&self) -> MutexGuard<'_, BTreeMap<String, VecDeque<Point>>> {
        self.series.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Append one sample to its series.
    pub fn push(&self, sample: Sample) {
        self.push_all([sample]);
    }

    /// Append samples. A sample for a new series is dropped when the server
    /// already holds [`MAX_SERIES`]; that is logged once until there is
    /// room again.
    pub fn push_all(&self, samples: impl IntoIterator<Item = Sample>) {
        // Series nobody reports any more go away without anyone asking.
        let now = Utc::now();
        let due = {
            let mut pruned = self.pruned.lock().unwrap_or_else(|p| p.into_inner());
            let due = pruned.is_none_or(|at| now - at >= PRUNE_EVERY);
            if due {
                *pruned = Some(now);
            }
            due
        };
        if due {
            self.prune(now);
        }
        let mut map = self.lock();
        for sample in samples {
            if !map.contains_key(&sample.metric) && map.len() >= MAX_SERIES {
                if !self.full.swap(true, Ordering::Relaxed) {
                    tracing::warn!(
                        max = MAX_SERIES,
                        "gauge: holding as many series as it keeps; samples for new ones are dropped"
                    );
                }
                continue;
            }
            let points = map.entry(sample.metric).or_default();
            while points.len() >= WINDOW_POINTS {
                points.pop_front();
            }
            points.push_back(Point {
                at: sample.timestamp,
                value: sample.value,
            });
        }
    }

    /// Drop points older than the window and series left with none.
    pub fn prune(&self, now: DateTime<Utc>) {
        let cutoff = now - WINDOW_AGE;
        let mut map = self.lock();
        map.retain(|_, points| {
            while points.front().is_some_and(|p| p.at < cutoff) {
                points.pop_front();
            }
            !points.is_empty()
        });
        if map.len() < MAX_SERIES {
            self.full.store(false, Ordering::Relaxed);
        }
    }

    pub fn series_count(&self) -> usize {
        self.lock().len()
    }

    /// Every point held, oldest first. `metric` keeps one series, by its
    /// exact key.
    pub fn list(&self, metric: Option<&str>) -> Vec<Sample> {
        let map = self.lock();
        let mut out: Vec<Sample> = map
            .iter()
            .filter(|(key, _)| metric.is_none_or(|m| key.as_str() == m))
            .flat_map(|(key, points)| {
                points.iter().map(|p| Sample {
                    metric: key.clone(),
                    value: p.value,
                    timestamp: p.at,
                })
            })
            .collect();
        out.sort_by_key(|s| s.timestamp);
        out
    }

    /// Latest sample of the series with exactly this key.
    pub fn latest(&self, metric: &str) -> Option<Sample> {
        let map = self.lock();
        let point = map.get(metric)?.back()?;
        Some(Sample {
            metric: metric.to_owned(),
            value: point.value,
            timestamp: point.at,
        })
    }

    /// The latest point of every series `keep` accepts, in key order.
    pub fn readings(&self, mut keep: impl FnMut(&str) -> bool) -> Vec<Reading> {
        let map = self.lock();
        map.iter()
            .filter(|(key, _)| keep(key))
            .filter_map(|(key, points)| {
                let last = points.back()?;
                let previous = points
                    .len()
                    .checked_sub(2)
                    .and_then(|i| points.get(i))
                    .map(|p| (p.at, p.value));
                Some(Reading {
                    metric: key.clone(),
                    value: last.value,
                    timestamp: last.at,
                    previous,
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
}
