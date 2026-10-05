// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Writes samples and alarms. The only place alarm `status` is updated.

use crate::store::SampleRing;
use crate::types::{
    phase_for, sanitize_metric, Alarm, AlarmSpec, AlarmStatus, Sample,
};
use chrono::{DateTime, Utc};
use pistack_store::{Collection, Store};
use pistack_types::{ApiError, ApiResult, Id, Meta};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::sync::Arc;

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

    pub fn ring(&self) -> &SampleRing {
        &self.samples
    }

    fn alarms(&self) -> Collection<Alarm> {
        self.store.collection()
    }

    /// Append a sample and re-evaluate every alarm watching `metric`.
    pub fn record_sample(&self, metric: impl Into<String>, value: f64) -> ApiResult<Sample> {
        let metric = metric.into().trim().to_owned();
        if metric.is_empty() || metric.len() > 256 {
            return Err(ApiError::bad_request("metric must be 1-256 characters"));
        }
        if !value.is_finite() {
            return Err(ApiError::bad_request("value must be finite"));
        }
        let sample = Sample {
            metric: metric.clone(),
            value,
            timestamp: Utc::now(),
        };
        self.samples.push(sample.clone());
        self.evaluate(&metric)?;
        Ok(sample)
    }

    /// Samples in arrival order. `metric` filters when set.
    pub fn list_samples(&self, metric: Option<&str>) -> Vec<Sample> {
        self.samples.list(metric)
    }

    fn latest(&self, metric: &str) -> Option<Sample> {
        self.samples.latest(metric)
    }

    /// Prometheus text exposition: one gauge per metric, the latest sample.
    /// Names are sanitised to `[a-zA-Z_:][a-zA-Z0-9_:]*`.
    pub fn metrics_text(&self) -> String {
        let buf = self.samples.lock();
        let mut latest: BTreeMap<String, (DateTime<Utc>, f64)> = BTreeMap::new();
        for sample in buf.iter() {
            let series = prom_series(&sample.metric);
            match latest.get(&series) {
                Some((ts, _)) if *ts > sample.timestamp => {}
                _ => {
                    latest.insert(series, (sample.timestamp, sample.value));
                }
            }
        }
        drop(buf);
        let mut out = String::new();
        let mut typed = BTreeSet::new();
        for (series, (_, value)) in &latest {
            let bare = series.split('{').next().unwrap_or(series);
            if typed.insert(bare.to_owned()) {
                let _ = writeln!(out, "# TYPE {bare} gauge");
            }
            let _ = writeln!(out, "{series} {}", format_value(*value));
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

    pub fn create_alarm(&self, meta: Meta, mut spec: AlarmSpec) -> ApiResult<Alarm> {
        spec.name = spec.name.trim().to_owned();
        spec.metric = spec.metric.trim().to_owned();
        validate_name(&meta.name)?;
        validate_alarm(&spec)?;
        let mut status = AlarmStatus::default();
        // `for_seconds` is kept on the spec. Phase follows the latest sample
        // immediately: crossing is Firing, otherwise Ok. Pending only when
        // this metric has no sample yet.
        if let Some(sample) = self.latest(&spec.metric) {
            status.last_value = Some(sample.value);
            status.phase = phase_for(spec.comparison, sample.value, spec.threshold);
        }
        self.alarms()
            .create(Alarm { meta, spec, status })
            .map_err(ApiError::from)
    }

    pub fn delete_alarm(&self, id: &Id) -> ApiResult<Alarm> {
        self.alarms().delete(id).map_err(ApiError::from)
    }

    /// Re-read the latest sample and write `status` on every alarm for it.
    fn evaluate(&self, metric: &str) -> ApiResult<()> {
        let Some(sample) = self.latest(metric) else {
            return Ok(());
        };
        let alarms = self.alarms().list(None)?;
        for alarm in alarms {
            if alarm.spec.metric != metric {
                continue;
            }
            let phase = phase_for(alarm.spec.comparison, sample.value, alarm.spec.threshold);
            if alarm.status.phase == phase && alarm.status.last_value == Some(sample.value) {
                continue;
            }
            let value = sample.value;
            let id = alarm.meta.id.clone();
            self.alarms()
                .modify(&id, |alarm| {
                    alarm.status.phase = phase;
                    alarm.status.last_value = Some(value);
                    Ok(())
                })
                .map_err(ApiError::from)?;
            tracing::debug!(
                alarm = %id,
                metric,
                ?phase,
                value,
                "alarm evaluated"
            );
        }
        Ok(())
    }
}

/// Host capacity: `pistack_host_memory_mib` and `pistack_host_vcpus`,
/// labeled by `node`. Does not re-evaluate alarms.
pub fn record_host(samples: &SampleRing, node: &str, memory_mib: u64, vcpus: u32) {
    let node = node_label(node);
    let now = Utc::now();
    samples.push(Sample {
        metric: format!("pistack_host_memory_mib{{node=\"{node}\"}}"),
        value: memory_mib as f64,
        timestamp: now,
    });
    samples.push(Sample {
        metric: format!("pistack_host_vcpus{{node=\"{node}\"}}"),
        value: vcpus as f64,
        timestamp: now,
    });
}

fn node_label(node: &str) -> String {
    let mut out: String = node
        .chars()
        .map(|c| match c {
            '"' | '\\' | '\n' | '\r' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    if out.is_empty() {
        out.push('_');
    }
    out
}

/// Prometheus series name. A `{labels}` suffix is kept; the name is sanitised.
fn prom_series(metric: &str) -> String {
    match metric.split_once('{') {
        Some((name, rest)) => {
            let name = sanitize_metric(name);
            let rest = rest.trim().strip_suffix('}').unwrap_or(rest.trim());
            if rest.is_empty()
                || rest
                    .chars()
                    .any(|c| c == '\n' || c == '\r' || c == '{' || c == '}')
            {
                name
            } else {
                format!("{name}{{{rest}}}")
            }
        }
        None => sanitize_metric(metric),
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
    if spec.metric.is_empty() || spec.metric.len() > 256 {
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
    use crate::types::MAX_SAMPLES;
    use pistack_store::Store;
    use std::sync::Arc;

    #[test]
    fn record_host_is_in_prometheus_text() {
        let samples = Arc::new(SampleRing::memory());
        record_host(&samples, "node-a", 2048, 4);
        let client = Client::new(Store::in_memory().unwrap(), samples);
        let text = client.metrics_text();
        assert!(text.contains("pistack_host_memory_mib"), "{text}");
        assert!(text.contains("pistack_host_vcpus"), "{text}");
        assert!(
            text.lines()
                .any(|l| l.contains("pistack_host_memory_mib") && l.contains("node=\"node-a\"")),
            "{text}"
        );
        assert!(
            text.lines().any(|l| l.contains("pistack_host_vcpus")
                && l.contains("node=\"node-a\"")
                && (l.ends_with(" 4") || l.ends_with(" 4.0"))),
            "{text}"
        );
    }

    #[test]
    fn max_samples_is_enforced() {
        let ring = SampleRing::memory();
        for i in 0..(MAX_SAMPLES + 5) {
            ring.push(Sample {
                metric: "m".into(),
                value: i as f64,
                timestamp: Utc::now(),
            });
        }
        assert_eq!(ring.list(None).len(), MAX_SAMPLES);
    }
}
