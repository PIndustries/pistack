// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! What hosts and machines are using: a heartbeat's usage report turned
//! into gauge series, and those series read back for one machine or host.
//!
//! The series live in gauge's window on the server that receives the
//! heartbeats, which is the leader. Nothing here is stored.

use crate::types::{Host, Machine};
use chrono::Utc;
use gauge::catalog as names;
use pistack_rpc::proto::{Heartbeat, HostUsage, MachineUsage};
use pistack_store::Store;
use serde::{Deserialize, Serialize};

/// A CPU reading older than this is not compared with: the machine may
/// have been away, and the average would say nothing about now.
const CPU_RATE_MAX_GAP_SECS: f64 = 300.0;

/// `GET /compute/machines/{id}/stats` and `GET /compute/hosts/{id}/stats`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Stats {
    /// Machine or host id.
    pub id: String,
    pub name: String,
    /// The host's node name; for a machine, the host it reports from.
    /// Empty when the machine has no series.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub host: String,
    /// Empty when nothing was reported in the last five minutes: the
    /// machine is off, its runtime does not measure, or its host is down.
    pub series: Vec<gauge::Stat>,
}

/// Record what one heartbeat says about a host and its machines.
pub fn record_heartbeat(gauge: &gauge::Client, store: &Store, host: &Host, hb: &Heartbeat) {
    let node = host.spec.node_name.as_str();
    let on_host = [("host", node)];
    let mut out = vec![
        (
            gauge::series(names::HOST_VCPUS, &on_host),
            f64::from(host.spec.capacity.vcpus),
        ),
        (
            gauge::series(names::HOST_MEMORY_MIB, &on_host),
            host.spec.capacity.memory_mib as f64,
        ),
        (
            gauge::series(names::HOST_MACHINES, &on_host),
            hb.machines.len() as f64,
        ),
    ];
    if let Some(usage) = &hb.usage {
        host_samples(&mut out, node, &usage.host);
        let machines = store.collection::<Machine>();
        for m in &usage.machines {
            // A guest the store does not know is not ours to report.
            if let Ok(Some(machine)) = machines.try_get(&m.id) {
                machine_samples(&mut out, gauge, node, &machine, m);
            }
        }
    }
    gauge.record_all(out);
}

fn host_samples(out: &mut Vec<(String, f64)>, node: &str, usage: &HostUsage) {
    let on_host = [("host", node)];
    let mut put = |name: &str, value: f64| out.push((gauge::series(name, &on_host), value));
    for (name, milli) in [names::HOST_LOAD1, names::HOST_LOAD5, names::HOST_LOAD15]
        .into_iter()
        .zip(usage.load_milli)
    {
        put(name, f64::from(milli) / 1000.0);
    }
    if let Some(total) = usage.memory_total_bytes {
        put(names::HOST_MEMORY_TOTAL, total as f64);
        if let Some(available) = usage.memory_available_bytes {
            put(
                names::HOST_MEMORY_USED,
                total.saturating_sub(available) as f64,
            );
        }
    }
    if let Some(total) = usage.data_disk_total_bytes {
        put(names::HOST_DISK_TOTAL, total as f64);
    }
    if let Some(free) = usage.data_disk_free_bytes {
        put(names::HOST_DISK_FREE, free as f64);
    }
}

fn machine_samples(
    out: &mut Vec<(String, f64)>,
    gauge: &gauge::Client,
    node: &str,
    machine: &Machine,
    usage: &MachineUsage,
) {
    let id = machine.meta.id.to_string();
    let project = machine
        .meta
        .project
        .as_ref()
        .map(ToString::to_string)
        .unwrap_or_default();
    let labels = [
        ("machine", id.as_str()),
        ("name", machine.meta.name.as_str()),
        ("project", project.as_str()),
        ("host", node),
    ];

    let cpu_key = gauge::series(names::MACHINE_CPU_SECONDS, &labels);
    let cpu_seconds = usage.cpu_ns as f64 / 1e9;
    // How many CPUs it kept busy since the report before this one.
    if let Some(before) = gauge.ring().latest(&cpu_key) {
        let secs = (Utc::now() - before.timestamp).num_milliseconds() as f64 / 1000.0;
        if secs > 0.0 && secs <= CPU_RATE_MAX_GAP_SECS && cpu_seconds >= before.value {
            out.push((
                gauge::series(names::MACHINE_CPU_CORES, &labels),
                (cpu_seconds - before.value) / secs,
            ));
        }
    }
    out.push((cpu_key, cpu_seconds));
    out.push((
        gauge::series(names::MACHINE_MEMORY, &labels),
        usage.memory_bytes as f64,
    ));
    if usage.memory_assigned_bytes > 0 {
        out.push((
            gauge::series(names::MACHINE_MEMORY_ASSIGNED, &labels),
            usage.memory_assigned_bytes as f64,
        ));
    }
    if let Some(used) = usage.memory_guest_used_bytes {
        out.push((
            gauge::series(names::MACHINE_MEMORY_GUEST_USED, &labels),
            used as f64,
        ));
    }
    for disk in &usage.disks {
        let mut on_disk = labels.to_vec();
        on_disk.push(("device", disk.device.as_str()));
        for (name, value) in [
            (names::MACHINE_DISK_READ_BYTES, disk.read_bytes),
            (names::MACHINE_DISK_WRITTEN_BYTES, disk.write_bytes),
            (names::MACHINE_DISK_READS, disk.read_ops),
            (names::MACHINE_DISK_WRITES, disk.write_ops),
        ] {
            out.push((gauge::series(name, &on_disk), value as f64));
        }
    }
    for nic in &usage.nics {
        let mut on_nic = labels.to_vec();
        on_nic.push(("interface", nic.name.as_str()));
        out.push((
            gauge::series(names::MACHINE_NET_RX_BYTES, &on_nic),
            nic.rx_bytes as f64,
        ));
        out.push((
            gauge::series(names::MACHINE_NET_TX_BYTES, &on_nic),
            nic.tx_bytes as f64,
        ));
    }
}

/// The current series of one machine.
pub fn machine_stats(gauge: &gauge::Client, machine: &Machine) -> Stats {
    let id = machine.meta.id.to_string();
    let series = gauge.stats(|s| s.labels.get("machine") == Some(&id));
    Stats {
        host: series
            .first()
            .and_then(|s| s.labels.get("host").cloned())
            .unwrap_or_default(),
        id,
        name: machine.meta.name.clone(),
        series,
    }
}

/// The current series of one host: its own, not its machines'.
pub fn host_stats(gauge: &gauge::Client, host: &Host) -> Stats {
    let node = &host.spec.node_name;
    Stats {
        id: host.meta.id.to_string(),
        name: host.meta.name.clone(),
        host: node.clone(),
        series: gauge
            .stats(|s| s.labels.get("host") == Some(node) && !s.labels.contains_key("machine")),
    }
}

#[cfg(test)]
mod tests {
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
}
