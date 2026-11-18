// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::Machine;
use chrono::Utc;
use gauge::catalog as names;
use pistack_rpc::proto::{Heartbeat, HostUsage, MachineUsage};
use pistack_store::Store;

use super::*;
use crate::types::{new_host, new_machine, MachineSpec};
use pistack_rpc::proto::{DiskUsage, UsageReport};
use pistack_types::{kind, Id, Resources};
use std::sync::Arc;

fn beat(node: &str, machines: Vec<MachineUsage>) -> Heartbeat {
    Heartbeat {
        node_name: node.into(),
        resources_free: Resources::default(),
        machines: Vec::new(),
        at: Utc::now(),
        ovn: None,
        usage: Some(UsageReport {
            host: HostUsage {
                load_milli: [1500, 250, 0],
                memory_total_bytes: Some(8 << 30),
                memory_available_bytes: Some(6 << 30),
                data_disk_total_bytes: Some(100 << 30),
                data_disk_free_bytes: Some(40 << 30),
            },
            machines,
        }),
    }
}

fn usage(id: &Id, cpu_ns: u64) -> MachineUsage {
    MachineUsage {
        id: id.clone(),
        cpu_ns,
        memory_bytes: 512 << 20,
        memory_assigned_bytes: 0,
        memory_guest_used_bytes: Some(100 << 20),
        disks: vec![DiskUsage {
            device: "vda".into(),
            read_bytes: 1,
            write_bytes: 2,
            read_ops: 3,
            write_ops: 4,
        }],
        nics: Vec::new(),
    }
}

#[test]
fn a_report_becomes_host_and_machine_series_and_cpu_use_needs_two() {
    let store = Store::in_memory().unwrap();
    let gauge = gauge::Client::new(store.clone(), Arc::new(gauge::SampleRing::memory()));
    let capacity = Resources {
        vcpus: 8,
        memory_mib: 8192,
        disk_gib: 100,
        gpus: 0,
    };
    let host = new_host("h1", "h1", capacity, vec![], Default::default());
    let project = Id::new(kind::PROJECT);
    let spec: MachineSpec =
        serde_json::from_value(serde_json::json!({"size": "s.1x1", "image": "img"})).unwrap();
    let machine = store
        .collection::<Machine>()
        .create(new_machine("web", Some(project.clone()), spec))
        .unwrap();
    let stranger = Id::new(kind::MACHINE);

    record_heartbeat(
        &gauge,
        &store,
        &host,
        &beat(
            "h1",
            vec![usage(&machine.meta.id, 10_000_000_000), usage(&stranger, 5)],
        ),
    );
    let of_host = host_stats(&gauge, &host);
    let value = |stats: &Stats, metric: &str| {
        stats
            .series
            .iter()
            .find(|s| s.metric == metric)
            .map(|s| s.value)
    };
    assert_eq!(value(&of_host, names::HOST_LOAD1), Some(1.5));
    assert_eq!(value(&of_host, names::HOST_LOAD5), Some(0.25));
    assert_eq!(
        value(&of_host, names::HOST_MEMORY_USED),
        Some((2u64 << 30) as f64)
    );
    assert_eq!(
        value(&of_host, names::HOST_DISK_FREE),
        Some((40u64 << 30) as f64)
    );
    assert_eq!(value(&of_host, names::HOST_VCPUS), Some(8.0));

    let first = machine_stats(&gauge, &machine);
    assert_eq!(first.host, "h1");
    assert_eq!(value(&first, names::MACHINE_CPU_SECONDS), Some(10.0));
    assert_eq!(
        value(&first, names::MACHINE_MEMORY_GUEST_USED),
        Some((100u64 << 20) as f64)
    );
    // No balloon figure, no series for it; and no CPU use from one report.
    assert_eq!(value(&first, names::MACHINE_MEMORY_ASSIGNED), None);
    assert_eq!(value(&first, names::MACHINE_CPU_CORES), None);
    assert_eq!(first.series[0].labels["project"], project.to_string());
    // A guest the store does not know left nothing behind.
    let unknown = stranger.to_string();
    assert!(gauge
        .stats(|s| s.labels.get("machine") == Some(&unknown))
        .is_empty());

    // Half a second of CPU later, however long that took, it is busy.
    std::thread::sleep(std::time::Duration::from_millis(50));
    record_heartbeat(
        &gauge,
        &store,
        &host,
        &beat("h1", vec![usage(&machine.meta.id, 10_500_000_000)]),
    );
    let second = machine_stats(&gauge, &machine);
    let cores = value(&second, names::MACHINE_CPU_CORES).expect("cpu use");
    assert!(cores > 0.0 && cores <= 0.5 / 0.05, "{cores}");
    // A counter that went backwards (the guest restarted) gives no
    // figure rather than a negative one.
    std::thread::sleep(std::time::Duration::from_millis(20));
    record_heartbeat(
        &gauge,
        &store,
        &host,
        &beat("h1", vec![usage(&machine.meta.id, 1_000_000_000)]),
    );
    let third = machine_stats(&gauge, &machine);
    assert_eq!(value(&third, names::MACHINE_CPU_CORES), Some(cores));
    assert_eq!(value(&third, names::MACHINE_CPU_SECONDS), Some(1.0));
}
