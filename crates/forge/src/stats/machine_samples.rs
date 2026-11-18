// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::Machine;
use chrono::Utc;
use gauge::catalog as names;
use pistack_rpc::proto::MachineUsage;

use super::cpu_rate_max_gap_secs::CPU_RATE_MAX_GAP_SECS;

pub(crate) fn machine_samples(
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
