// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Writes samples and alarms. The only place alarm `status` is updated.

use crate::catalog::{self, MetricKind, RESERVED_PREFIX};
use crate::series::{sanitize_name, Series};
use crate::store::SampleRing;
use crate::types::{
    crosses, decide_on, hold_coverable, phase_for, Alarm, AlarmPhase, AlarmSpec, AlarmSpecUpdate,
    AlarmStatus, Comparison, Sample, ALARM_REFRESH, MAX_HOLD, STALE_AFTER,
};
use chrono::{DateTime, Utc};
use pistack_store::{Collection, Store};
use pistack_types::{ApiError, ApiResult, Id, Meta};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::Arc;

/// Store meta key gate keeps the admin project's id under. An alarm in
/// that project, like one in no project, may watch any series.
const META_ADMIN_PROJECT: &str = "gate/admin_project";

#[derive(Clone)]
pub struct Client {
    store: Store,
    samples: Arc<SampleRing>,
}

/// One series an alarm's metric selects, with the points the window holds for it.
struct Choice {
    series: String,
    /// The series' points, oldest stamp first. The newest of them is the point an alarm is
    /// decided from, read from the window rather than trusted from a reading.
    points: Vec<Sample>,
    /// The newest point, and whether it is no older than [`STALE_AFTER`]. Freshness is the
    /// wall clock's judgement and only an alarm with no hold takes it: a series that
    /// stopped reporting keeps the phase its points last decided.
    newest: (DateTime<Utc>, f64),
    fresh: bool,
}

impl Choice {
    fn value(&self) -> f64 {
        self.newest.1
    }
}

/// One series as it stands now.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Stat {
    pub metric: String,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    pub value: f64,
    pub timestamp: DateTime<Utc>,
    /// For a counter: how fast it rose between the last two samples, per
    /// second. Absent for a gauge and until there are two samples.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rate: Option<f64>,
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

fn format_value(value: f64) -> String {
    if value.is_nan() {
        return "NaN".into();
    }
    if value.is_infinite() {
        return if value.is_sign_positive() {
            "+Inf".into()
        } else {
            "-Inf".into()
        };
    }
    format!("{value}")
}

fn validate_name(name: &str) -> ApiResult<()> {
    if name.trim().is_empty() || name.len() > 128 || name.contains('/') {
        return Err(ApiError::bad_request(
            "name must be 1-128 characters without '/'",
        ));
    }
    Ok(())
}

fn validate_alarm(spec: &AlarmSpec) -> ApiResult<()> {
    validate_name(&spec.name)?;
    if spec.metric.trim().is_empty() || spec.metric.len() > 256 {
        return Err(ApiError::bad_request("metric must be 1-256 characters"));
    }
    if !spec.threshold.is_finite() {
        return Err(ApiError::bad_request("threshold must be finite"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::series::series;
    use crate::types::AlarmPhase;
    use pistack_types::kind;

    fn client() -> Client {
        Client::new(Store::in_memory().unwrap(), Arc::new(SampleRing::memory()))
    }

    /// Read the text format back: every sample line with its value, and the
    /// type each metric name was declared with.
    fn parse_exposition(text: &str) -> (BTreeMap<String, f64>, BTreeMap<String, String>) {
        let mut samples = BTreeMap::new();
        let mut types = BTreeMap::new();
        let mut helped = std::collections::BTreeSet::new();
        for line in text.lines() {
            if let Some(rest) = line.strip_prefix("# TYPE ") {
                let (name, kind) = rest.split_once(' ').expect("a type");
                assert!(matches!(kind, "gauge" | "counter"), "{line}");
                assert!(
                    types.insert(name.to_owned(), kind.to_owned()).is_none(),
                    "{name} is typed twice"
                );
                continue;
            }
            if let Some(rest) = line.strip_prefix("# HELP ") {
                let (name, help) = rest.split_once(' ').expect("help text");
                assert!(!help.is_empty());
                assert!(!types.contains_key(name), "HELP after TYPE for {name}");
                assert!(helped.insert(name.to_owned()), "{name} has help twice");
                continue;
            }
            assert!(!line.starts_with('#') && !line.is_empty(), "{line:?}");
            let (series, value) = line.rsplit_once(' ').expect("a value");
            let value: f64 = value.parse().unwrap_or_else(|_| panic!("{line}"));
            let parsed = Series::parse(series);
            // The line is in canonical form and its name is legal and typed
            // before its first sample.
            assert_eq!(parsed.exposition(), series, "{line}");
            assert_eq!(sanitize_name(&parsed.name), parsed.name, "{line}");
            assert!(types.contains_key(&parsed.name), "no TYPE before {line}");
            assert!(
                samples.insert(series.to_owned(), value).is_none(),
                "{series} is there twice"
            );
        }
        (samples, types)
    }

    #[test]
    fn the_exposition_reads_back_with_names_labels_types_and_values() {
        let c = client();
        let machine = [
            ("machine", "mach-1"),
            ("name", "we\"b"),
            ("project", "proj-1"),
            ("host", "h1"),
        ];
        c.record_all(vec![
            (series(catalog::HOST_LOAD1, &[("host", "h1")]), 0.25),
            (series(catalog::HOST_LOAD1, &[("host", "h2")]), 3.0),
            (series(catalog::MACHINE_MEMORY, &machine), 1024.0),
            (series(catalog::MACHINE_CPU_SECONDS, &machine), 12.5),
            ("custom.metric".to_owned(), 80.0),
            ("skipped".to_owned(), f64::NAN),
        ]);
        let text = c.metrics_text();
        let (samples, types) = parse_exposition(&text);

        assert_eq!(samples["pistack_host_load1{host=\"h1\"}"], 0.25);
        assert_eq!(samples["pistack_host_load1{host=\"h2\"}"], 3.0);
        let memory = "pistack_machine_memory_bytes{host=\"h1\",machine=\"mach-1\",name=\"we\\\"b\",project=\"proj-1\"}";
        assert_eq!(samples[memory], 1024.0, "{text}");
        assert_eq!(samples["custom_metric"], 80.0);
        assert_eq!(samples[catalog::SERIES], 5.0);
        assert!(!text.contains("skipped"));

        assert_eq!(types["pistack_host_load1"], "gauge");
        assert_eq!(types["pistack_machine_cpu_seconds_total"], "counter");
        assert_eq!(types["custom_metric"], "gauge");
        assert!(text.contains("# HELP pistack_host_load1 "), "{text}");
        assert!(!text.contains("# HELP custom_metric"), "{text}");
    }

    #[test]
    fn a_series_that_stopped_reporting_leaves_the_exposition() {
        let c = client();
        let gone = series(catalog::HOST_LOAD1, &[("host", "gone")]);
        c.ring().push(Sample {
            metric: gone.clone(),
            value: 1.0,
            timestamp: Utc::now() - STALE_AFTER - chrono::Duration::seconds(1),
        });
        c.record_all(vec![(
            series(catalog::HOST_LOAD1, &[("host", "here")]),
            1.0,
        )]);
        let text = c.metrics_text();
        assert!(text.contains("host=\"here\""), "{text}");
        assert!(!text.contains("host=\"gone\""), "{text}");
        assert!(c
            .stats(|s| s.labels.get("host").is_some_and(|h| h == "gone"))
            .is_empty());
        // The window still has it for the samples API.
        assert_eq!(c.list_samples(Some(&gone)).len(), 1);
    }

    #[test]
    fn stats_give_a_rate_for_counters_only() {
        let c = client();
        let cpu = series(catalog::MACHINE_CPU_SECONDS, &[("machine", "m")]);
        let mem = series(catalog::MACHINE_MEMORY, &[("machine", "m")]);
        let earlier = Utc::now() - chrono::Duration::seconds(10);
        for key in [&cpu, &mem] {
            c.ring().push(Sample {
                metric: key.clone(),
                value: 100.0,
                timestamp: earlier,
            });
        }
        c.record_all(vec![(cpu, 120.0), (mem, 200.0)]);
        let stats = c.stats(|s| s.labels.get("machine").is_some_and(|m| m == "m"));
        assert_eq!(stats.len(), 2);
        let cpu = stats
            .iter()
            .find(|s| s.metric == catalog::MACHINE_CPU_SECONDS)
            .unwrap();
        let rate = cpu.rate.expect("a rate");
        assert!((1.9..=2.1).contains(&rate), "{rate}");
        let mem = stats
            .iter()
            .find(|s| s.metric == catalog::MACHINE_MEMORY)
            .unwrap();
        assert_eq!((mem.value, mem.rate), (200.0, None));
    }

    fn alarm(c: &Client, project: Option<Id>, metric: &str, cmp: Comparison, at: f64) -> Alarm {
        let name = format!("a-{}", uuid::Uuid::now_v7());
        c.create_alarm(
            Meta::new(kind::ALARM, name.clone(), project),
            AlarmSpec {
                name,
                metric: metric.into(),
                comparison: cmp,
                threshold: at,
                for_seconds: 0,
            },
        )
        .unwrap()
    }

    fn status(c: &Client, a: &Alarm) -> AlarmStatus {
        c.get_alarm(None, &a.meta.id.to_string()).unwrap().status
    }

    #[test]
    fn an_alarm_on_a_machine_series_fires_and_clears() {
        let c = client();
        let cores = |machine: &str| {
            series(
                catalog::MACHINE_CPU_CORES,
                &[("machine", machine), ("project", "proj-1"), ("host", "h1")],
            )
        };
        // One alarm on one machine, one on every machine.
        let one = alarm(
            &c,
            None,
            "pistack_machine_cpu_usage_cores{machine=\"mach-1\"}",
            Comparison::Gt,
            1.5,
        );
        let any = alarm(&c, None, catalog::MACHINE_CPU_CORES, Comparison::Gt, 1.5);
        assert_eq!(status(&c, &one).phase, AlarmPhase::Pending);

        c.record_all(vec![(cores("mach-1"), 0.2), (cores("mach-2"), 0.4)]);
        assert_eq!(status(&c, &one).phase, AlarmPhase::Ok);
        assert_eq!(status(&c, &one).last_value, Some(0.2));
        assert_eq!(status(&c, &any).phase, AlarmPhase::Ok);

        // The other machine gets busy: only the alarm on every machine fires,
        // and it says which series did it.
        c.record_all(vec![(cores("mach-1"), 0.3), (cores("mach-2"), 1.9)]);
        assert_eq!(status(&c, &one).phase, AlarmPhase::Ok);
        let fired = status(&c, &any);
        assert_eq!(fired.phase, AlarmPhase::Firing);
        assert_eq!(fired.last_value, Some(1.9));
        assert_eq!(fired.series, Some(cores("mach-2")));

        c.record_all(vec![(cores("mach-1"), 2.5), (cores("mach-2"), 0.1)]);
        assert_eq!(status(&c, &one).phase, AlarmPhase::Firing);
        assert_eq!(status(&c, &any).phase, AlarmPhase::Firing);
        c.record_all(vec![(cores("mach-1"), 0.1), (cores("mach-2"), 0.1)]);
        assert_eq!(status(&c, &one).phase, AlarmPhase::Ok);
        assert_eq!(status(&c, &any).phase, AlarmPhase::Ok);
    }

    #[test]
    fn a_steady_alarm_is_not_rewritten_for_every_sample() {
        let c = client();
        let load = series(catalog::HOST_LOAD1, &[("host", "h1")]);
        c.record_all(vec![(load.clone(), 1.0)]);
        let a = alarm(&c, None, catalog::HOST_LOAD1, Comparison::Gt, 10.0);
        let before = c.get_alarm(None, &a.meta.id.to_string()).unwrap();
        c.record_all(vec![(load.clone(), 2.0)]);
        c.record_all(vec![(load.clone(), 3.0)]);
        let after = c.get_alarm(None, &a.meta.id.to_string()).unwrap();
        assert_eq!(after.meta.revision, before.meta.revision);
        // A change of phase is written at once.
        c.record_all(vec![(load, 11.0)]);
        let fired = c.get_alarm(None, &a.meta.id.to_string()).unwrap();
        assert_eq!(fired.status.phase, AlarmPhase::Firing);
        assert_eq!(fired.status.last_value, Some(11.0));
    }

    #[test]
    fn a_project_alarm_sees_only_its_own_machines() {
        let c = client();
        let mine = Id::new(kind::PROJECT);
        let admin = Id::new(kind::PROJECT);
        c.store().put_meta(META_ADMIN_PROJECT, &admin).unwrap();
        let memory = |machine: &str, project: &str| {
            series(
                catalog::MACHINE_MEMORY,
                &[("machine", machine), ("project", project)],
            )
        };
        c.record_all(vec![
            (memory("theirs", "proj-other"), 900.0),
            (memory("mine", &mine.to_string()), 100.0),
            (series(catalog::HOST_LOAD1, &[("host", "h1")]), 50.0),
        ]);
        let over = |project: Option<Id>, metric: &str| {
            alarm(&c, project, metric, Comparison::Gt, 500.0).status
        };
        // The project's alarm is decided by its own machine alone, and it
        // cannot watch a host or name another project's machine.
        let own = over(Some(mine.clone()), catalog::MACHINE_MEMORY);
        assert_eq!((own.phase, own.last_value), (AlarmPhase::Ok, Some(100.0)));
        let named = "pistack_machine_memory_bytes{machine=\"theirs\"}";
        assert_eq!(over(Some(mine.clone()), named).phase, AlarmPhase::Pending);
        assert_eq!(
            over(Some(mine), catalog::HOST_LOAD1).phase,
            AlarmPhase::Pending
        );
        // An admin's alarm, in the admin project or in none, sees all.
        assert_eq!(
            over(Some(admin), catalog::MACHINE_MEMORY).phase,
            AlarmPhase::Firing
        );
        assert_eq!(over(None, named).last_value, Some(900.0));
    }
}
