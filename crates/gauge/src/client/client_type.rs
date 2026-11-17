// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::catalog::{self, MetricKind, RESERVED_PREFIX};
use crate::series::{sanitize_name, Series};
use crate::store::SampleRing;
use crate::types::{
    crosses, decide_on, hold_coverable, phase_for, Alarm, AlarmPhase, AlarmSpec, AlarmSpecUpdate,
    AlarmStatus, Comparison, Sample, ALARM_REFRESH, MAX_HOLD, STALE_AFTER,
};
use chrono::Utc;
use pistack_store::{Collection, Store};
use pistack_types::{ApiError, ApiResult, Id, Meta};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::Arc;

use super::choice::Choice;
use super::format_value::format_value;
use super::stat::Stat;
use super::validate_alarm::validate_alarm;
use super::validate_name::validate_name;

/// that project, like one in no project, may watch any series.
pub(crate) const META_ADMIN_PROJECT: &str = "gate/admin_project";

#[derive(Clone)]
pub struct Client {
    store: Store,
    samples: Arc<SampleRing>,
}

impl Client {
    pub fn new(store: Store, samples: Arc<SampleRing>) -> Self {
        Self { store, samples }
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    /// The window this client writes into.
    pub fn ring(&self) -> &Arc<SampleRing> {
        &self.samples
    }

    fn alarms(&self) -> Collection<Alarm> {
        self.store.collection()
    }

    /// Append a sample stamped now and re-evaluate the alarms watching its series.
    pub fn record_sample(&self, metric: impl Into<String>, value: f64) -> ApiResult<Sample> {
        let metric = metric.into().trim().to_owned();
        self.record(Sample {
            metric,
            value,
            timestamp: Utc::now(),
        })
    }

    /// Append `sample` and re-evaluate the alarms watching its series. For a caller
    /// reporting a reading it took at a moment of its own, which is what a server's own
    /// measurements are. A stamp beyond the window the series keeps is dropped, as any
    /// sample of a series nobody reports any more is.
    pub fn record(&self, sample: Sample) -> ApiResult<Sample> {
        if sample.metric.is_empty() || sample.metric.len() > 256 {
            return Err(ApiError::bad_request("metric must be 1-256 characters"));
        }
        if !sample.value.is_finite() {
            return Err(ApiError::bad_request("value must be finite"));
        }
        self.samples.push(sample.clone());
        self.evaluate(std::slice::from_ref(&Series::parse(&sample.metric)));
        Ok(sample)
    }

    /// Record what the server measured itself: `(series key, value)` pairs,
    /// all at this moment. Alarms on those series are re-evaluated. Values
    /// that are not finite are left out. Nothing here fails the caller: an
    /// alarm that could not be written is logged.
    pub fn record_all(&self, samples: Vec<(String, f64)>) {
        let now = Utc::now();
        let samples: Vec<Sample> = samples
            .into_iter()
            .filter(|(_, value)| value.is_finite())
            .map(|(metric, value)| Sample {
                metric,
                value,
                timestamp: now,
            })
            .collect();
        let changed: Vec<Series> = samples.iter().map(|s| Series::parse(&s.metric)).collect();
        self.samples.push_all(samples);
        self.evaluate(&changed);
    }

    /// Every point held. `metric` keeps one series by its key.
    pub fn list_samples(&self, metric: Option<&str>) -> Vec<Sample> {
        self.samples.list(metric)
    }

    /// The current value of every series `keep` accepts that has had a
    /// sample in the last [`STALE_AFTER`].
    pub fn stats(&self, keep: impl Fn(&Series) -> bool) -> Vec<Stat> {
        let now = Utc::now();
        self.samples
            .readings(|_| true)
            .into_iter()
            .filter(|r| now - r.timestamp <= STALE_AFTER)
            .filter_map(|r| {
                let series = Series::parse(&r.metric);
                if !keep(&series) {
                    return None;
                }
                let counter =
                    catalog::info(&series.name).is_some_and(|m| m.kind == MetricKind::Counter);
                Some(Stat {
                    rate: if counter { r.rate() } else { None },
                    metric: series.name,
                    labels: series.labels,
                    value: r.value,
                    timestamp: r.timestamp,
                })
            })
            .collect()
    }

    /// Prometheus text exposition: the latest value of every series that
    /// has had a sample in the last [`STALE_AFTER`]. PiStack's own metrics
    /// come with their help text and type; anything else is a gauge.
    pub fn metrics_text(&self) -> String {
        let now = Utc::now();
        self.samples.prune(now);
        let mut families: BTreeMap<String, BTreeMap<String, f64>> = BTreeMap::new();
        for r in self.samples.readings(|_| true) {
            if now - r.timestamp > STALE_AFTER {
                continue;
            }
            let series = Series::parse(&r.metric);
            families
                .entry(sanitize_name(&series.name))
                .or_default()
                .insert(series.exposition(), r.value);
        }
        families
            .entry(catalog::SERIES.to_owned())
            .or_default()
            .insert(
                catalog::SERIES.to_owned(),
                self.samples.series_count() as f64,
            );
        let mut out = String::new();
        for (name, lines) in &families {
            let info = catalog::info(name);
            if let Some(info) = info {
                let _ = writeln!(out, "# HELP {name} {}", info.help);
            }
            let kind = info.map_or(MetricKind::Gauge, |i| i.kind);
            let _ = writeln!(out, "# TYPE {name} {}", kind.as_str());
            for (series, value) in lines {
                let _ = writeln!(out, "{series} {}", format_value(*value));
            }
        }
        out
    }

    pub fn list_alarms(&self, project: Option<&Id>) -> ApiResult<Vec<Alarm>> {
        self.alarms().list(project).map_err(ApiError::from)
    }

    pub fn get_alarm(&self, project: Option<&Id>, name_or_id: &str) -> ApiResult<Alarm> {
        self.alarms()
            .resolve(project, name_or_id)
            .map_err(ApiError::from)
    }

    /// The points the window holds for one series, oldest stamp first.
    fn held(&self, metric: &str) -> Vec<Sample> {
        self.samples
            .list(Some(metric))
            .into_iter()
            .filter(|s| s.metric == metric)
            .collect()
    }

    pub fn create_alarm(&self, meta: Meta, mut spec: AlarmSpec) -> ApiResult<Alarm> {
        spec.name = spec.name.trim().to_owned();
        spec.metric = spec.metric.trim().to_owned();
        validate_name(&meta.name)?;
        validate_alarm(&spec)?;
        // A hold is measured on the stamps of the points a series keeps, so one longer
        // than the window reaches could never be spanned by any run of them.
        if !hold_coverable(spec.for_seconds) {
            return Err(ApiError::unprocessable(format!(
                "for_seconds must be at most {}, the longest hold the sample window can \
                 decide",
                MAX_HOLD.num_seconds()
            )));
        }
        let mut status = AlarmStatus::default();
        // Without a `for_seconds`, phase follows the latest sample at once: crossing is
        // Firing, otherwise Ok, and Pending while no series the alarm selects has a fresh
        // sample. An alarm with a hold is decided on the stamps of the samples, so it is
        // decided from the window on which it is made and not from a point that has to
        // arrive: a value crossing for a minute before the alarm existed has stamps
        // saying so.
        let held = spec.for_seconds > 0;
        if let Some(chosen) = self
            .select(&spec, meta.project.as_ref(), &self.admin_project())
            .iter()
            .rev()
            .find(|c| held || c.fresh)
        {
            status.last_value = Some(chosen.value());
            status.series = Some(chosen.series.clone());
            status.phase = if held {
                decide_on(&spec, &chosen.points)
            } else {
                phase_for(spec.comparison, chosen.value(), spec.threshold)
            };
        }
        self.alarms()
            .create(Alarm { meta, spec, status })
            .map_err(ApiError::from)
    }

    /// Replace an alarm. The hold of an alarm is measured on the stamps of the samples
    /// there now, so the one this alarm asks for is decided from the window as soon as
    /// anything next decides it; nothing of the alarm it replaced carries over, because
    /// nothing was ever counted for it.
    pub fn update_alarm(&self, alarm: Alarm, expected_revision: Option<u64>) -> ApiResult<Alarm> {
        validate_alarm(&alarm.spec)?;
        if !hold_coverable(alarm.spec.for_seconds) {
            return Err(ApiError::unprocessable(format!(
                "for_seconds must be at most {}, the longest hold the sample window can \
                 decide",
                MAX_HOLD.num_seconds()
            )));
        }
        self.alarms()
            .update(alarm, expected_revision)
            .map_err(ApiError::from)
    }

    /// Change what an alarm watches, or its description and labels. What the
    /// update names is changed; the rest stays. A new spec goes through the
    /// same validation as a replacement, and is decided again at once from the
    /// window the way a creation would be: what the samples are measured
    /// against is new, so nothing of the old alarm's run carries over.
    pub fn update_alarm_fields(
        &self,
        project: Option<&Id>,
        name_or_id: &str,
        description: Option<String>,
        labels: Option<BTreeMap<String, String>>,
        spec: Option<AlarmSpecUpdate>,
    ) -> ApiResult<Alarm> {
        let current = self.get_alarm(project, name_or_id)?;
        let mut next = current.clone();
        if let Some(d) = &description {
            next.meta.description = d.clone();
        }
        if let Some(l) = &labels {
            next.meta.labels = l.clone();
        }
        let watched = match spec {
            Some(update) => {
                let candidate = update.apply(current.spec.clone());
                let changed = candidate != current.spec;
                next.spec = candidate;
                changed
            }
            None => false,
        };
        if !watched {
            return self.update_alarm(next, None);
        }
        validate_alarm(&next.spec)?;
        if !hold_coverable(next.spec.for_seconds) {
            return Err(ApiError::unprocessable(format!(
                "for_seconds must be at most {}, the longest hold the sample window can \
                 decide",
                MAX_HOLD.num_seconds()
            )));
        }
        // Decided as at a creation: an alarm with a hold from the stamps held,
        // one without from a series still reporting. What no series says is
        // left Pending, and what none says at all has nothing to show.
        let mut status = AlarmStatus::default();
        let held = next.spec.for_seconds > 0;
        if let Some(chosen) = self
            .select(
                &next.spec,
                next.meta.project.as_ref(),
                &self.admin_project(),
            )
            .iter()
            .rev()
            .find(|c| held || c.fresh)
        {
            status.last_value = Some(chosen.value());
            status.series = Some(chosen.series.clone());
            status.phase = if held {
                decide_on(&next.spec, &chosen.points)
            } else {
                phase_for(next.spec.comparison, chosen.value(), next.spec.threshold)
            };
        }
        next.status = status;
        self.update_alarm(next, None)
    }

    pub fn delete_alarm(&self, id: &Id) -> ApiResult<Alarm> {
        self.alarms().delete(id).map_err(ApiError::from)
    }

    fn admin_project(&self) -> Option<Id> {
        self.store.get_meta::<Id>(META_ADMIN_PROJECT).ok().flatten()
    }

    /// The series an alarm is decided from: among those its metric selects, the one that
    /// crosses the furthest, or comes closest. An alarm that belongs to a project only
    /// sees PiStack's own series for that project, so it cannot be used to read another's
    /// machines.
    fn select(
        &self,
        spec: &AlarmSpec,
        project: Option<&Id>,
        admin_project: &Option<Id>,
    ) -> Vec<Choice> {
        let selector = Series::parse(&spec.metric);
        let own = project
            .filter(|p| Some(*p) != admin_project.as_ref())
            .map(Id::to_string);
        let now = Utc::now();
        let mut candidates: Vec<Choice> = self
            .samples
            .readings(|key| key.starts_with(selector.name.as_str()))
            .into_iter()
            .filter(|r| {
                let series = Series::parse(&r.metric);
                selector.selects(&series)
                    && match &own {
                        Some(p) if series.name.starts_with(RESERVED_PREFIX) => {
                            series.labels.get("project") == Some(p)
                        }
                        _ => true,
                    }
            })
            .map(|r| {
                let points = self.held(&r.metric);
                let newest = match points.last() {
                    Some(p) => (p.timestamp, p.value),
                    None => (r.timestamp, r.value),
                };
                Choice {
                    fresh: now - newest.0 <= STALE_AFTER,
                    points,
                    newest,
                    series: r.metric,
                }
            })
            .collect();
        match spec.comparison {
            Comparison::Gt => candidates.sort_by(|a, b| a.value().total_cmp(&b.value())),
            Comparison::Lt => candidates.sort_by(|a, b| b.value().total_cmp(&a.value())),
            // Any series at the threshold decides; otherwise the freshest.
            Comparison::Eq => candidates.sort_by_key(|c| {
                (
                    crosses(Comparison::Eq, c.value(), spec.threshold),
                    c.fresh,
                    c.newest.0,
                )
            }),
        }
        candidates
    }

    /// Re-evaluate the alarms whose metric selects one of `changed`.
    ///
    /// A hold is measured on the stamps of the points a series keeps, so the points held
    /// are what say how long a run of crossings has run. Nothing about a hold is kept
    /// apart from those points: a server that restarts decides from the same stamps as the
    /// one that died, and two servers holding the same window decide the same thing.
    ///
    /// Status is written when the phase changes. While it stays the same,
    /// a new `last_value` is written at most every [`ALARM_REFRESH`], so an
    /// alarm on a busy series is not a store write per sample.
    fn evaluate(&self, changed: &[Series]) {
        if changed.is_empty() {
            return;
        }
        // Read afresh: what an alarm asks for now is what this sample is decided against,
        // and an update landing since the last sample is exactly what should.
        let alarms = match self.alarms().list(None) {
            Ok(alarms) => alarms,
            // A follower cannot read the record of an alarm: the leader decides it.
            Err(e) => {
                tracing::debug!(error = %e, "gauge: alarms not evaluated");
                return;
            }
        };
        if alarms.is_empty() {
            return;
        }
        let admin_project = self.admin_project();
        let now = Utc::now();
        for alarm in alarms {
            let selector = Series::parse(&alarm.spec.metric);
            if !changed.iter().any(|series| selector.selects(series)) {
                continue;
            }
            // Among the series the metric selects, the one that crosses furthest, or
            // comes closest. An alarm with no hold asks one still reporting, so a series
            // that stopped reporting keeps its phase; an alarm with a hold is decided from
            // the stamps there are, and nothing is read from the wall clock to ask whether
            // they are old, because a sample that arrives late is still the sample it was.
            let held = alarm.spec.for_seconds > 0;
            let selection = self.select(&alarm.spec, alarm.meta.project.as_ref(), &admin_project);
            let Some(chosen) = selection.iter().rev().find(|c| held || c.fresh) else {
                continue;
            };
            let series = chosen.series.clone();
            let value = chosen.value();
            let phase = if held {
                decide_on(&alarm.spec, &chosen.points)
            } else {
                phase_for(alarm.spec.comparison, value, alarm.spec.threshold)
            };
            // What a sample says to the store is its value, so that is what is compared
            // before anything is written; while the phase stays the same the value is
            // refreshed at most every `ALARM_REFRESH`, except when the sample crosses at a
            // value the record has never held while the alarm is held for, which is the
            // wait being seen to run and is written.
            let same_value = alarm.status.last_value == Some(value);
            let recent = now - alarm.meta.updated_at < ALARM_REFRESH;
            let waiting = held
                && phase == AlarmPhase::Ok
                && crosses(alarm.spec.comparison, value, alarm.spec.threshold);
            if alarm.status.phase == phase && (same_value || (recent && !waiting)) {
                continue;
            }
            let id = alarm.meta.id.clone();
            if let Err(e) = self.alarms().modify(&id, |alarm| {
                alarm.status.phase = phase;
                alarm.status.last_value = Some(value);
                alarm.status.series = Some(series.clone());
                Ok(())
            }) {
                // A follower cannot write alarm status; the leader will.
                tracing::debug!(alarm = %id, error = %e, "gauge: alarm status not written");
                continue;
            }
            tracing::debug!(alarm = %id, series = %series, ?phase, value, "alarm evaluated");
        }
    }
}
