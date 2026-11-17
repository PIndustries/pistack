// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::catalog;
use crate::series::{sanitize_name, Series};
use crate::store::SampleRing;
use crate::types::{Alarm, AlarmPhase, AlarmSpec, AlarmStatus, Comparison, Sample, STALE_AFTER};
use chrono::Utc;
use pistack_store::Store;
use pistack_types::{Id, Meta};
use std::collections::BTreeMap;
use std::sync::Arc;

use super::client_type::Client;
use super::client_type::META_ADMIN_PROJECT;
use crate::series::series;
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
