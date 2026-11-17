// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::{Sample, MAX_SERIES, WINDOW_AGE, WINDOW_POINTS};
use chrono::{DateTime, Utc};
use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard};

use super::point::Point;
use super::reading::Reading;

/// Per-series windows of samples. The name is from when this was one ring.
#[derive(Debug, Default)]
pub struct SampleRing {
    series: Mutex<BTreeMap<String, VecDeque<Point>>>,
    full: AtomicBool,
    /// When old points were last dropped.
    pruned: Mutex<Option<DateTime<Utc>>>,
}

/// How often a push also drops what has aged out of the window.
pub(crate) const PRUNE_EVERY: chrono::Duration = chrono::Duration::seconds(60);

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
            // Kept oldest stamp first, so the last point is the newest however the samples
            // for a series arrive: a report carrying a reading it took earlier is a point
            // of the series and does not become the point alarms read. What leaves the
            // window is still what a server that receives its samples in order reports
            // first.
            let at = points.partition_point(|p| p.at <= sample.timestamp);
            points.insert(
                at,
                Point {
                    at: sample.timestamp,
                    value: sample.value,
                },
            );
            while points.len() > WINDOW_POINTS {
                points.pop_front();
            }
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

    /// Every point held. `metric` keeps one series, by its exact key. Points
    /// come out oldest first within a series; across series the order is by
    /// key and not by time.
    pub fn list(&self, metric: Option<&str>) -> Vec<Sample> {
        let map = self.lock();
        map.iter()
            .filter(|(key, _)| metric.is_none_or(|m| key.as_str() == m))
            .flat_map(|(key, points)| {
                points.iter().map(|p| Sample {
                    metric: key.clone(),
                    value: p.value,
                    timestamp: p.at,
                })
            })
            .collect()
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
