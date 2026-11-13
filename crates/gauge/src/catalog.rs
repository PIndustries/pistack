// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The metrics PiStack records itself: their names, what kind each is and
//! what it means. The names and labels are part of the interface; dashboards
//! and alert rules are written against them, so they do not change.
//!
//! Host series carry `host`. Machine series carry `machine` (the id),
//! `name`, `project` (the id) and `host`; disk series add `device` and
//! network series add `interface`. Cluster series carry `server`, the id of
//! the server that reports them.

/// Every name here starts with this. A sample posted through the API may
/// not use it.
pub const RESERVED_PREFIX: &str = "pistack_";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MetricKind {
    Gauge,
    /// Only ever goes up, until the thing it counts restarts.
    Counter,
}

impl MetricKind {
    pub fn as_str(self) -> &'static str {
        match self {
            MetricKind::Gauge => "gauge",
            MetricKind::Counter => "counter",
        }
    }
}

pub struct MetricInfo {
    pub name: &'static str,
    pub kind: MetricKind,
    pub help: &'static str,
}

pub const HOST_VCPUS: &str = "pistack_host_vcpus";
pub const HOST_MEMORY_MIB: &str = "pistack_host_memory_mib";
pub const HOST_LOAD1: &str = "pistack_host_load1";
pub const HOST_LOAD5: &str = "pistack_host_load5";
pub const HOST_LOAD15: &str = "pistack_host_load15";
pub const HOST_MEMORY_USED: &str = "pistack_host_memory_used_bytes";
pub const HOST_MEMORY_TOTAL: &str = "pistack_host_memory_total_bytes";
pub const HOST_DISK_FREE: &str = "pistack_host_data_disk_free_bytes";
pub const HOST_DISK_TOTAL: &str = "pistack_host_data_disk_total_bytes";
pub const HOST_MACHINES: &str = "pistack_host_machines";

pub const MACHINE_CPU_SECONDS: &str = "pistack_machine_cpu_seconds_total";
pub const MACHINE_CPU_CORES: &str = "pistack_machine_cpu_usage_cores";
pub const MACHINE_MEMORY: &str = "pistack_machine_memory_bytes";
pub const MACHINE_MEMORY_ASSIGNED: &str = "pistack_machine_memory_assigned_bytes";
pub const MACHINE_MEMORY_GUEST_USED: &str = "pistack_machine_memory_guest_used_bytes";
pub const MACHINE_DISK_READ_BYTES: &str = "pistack_machine_disk_read_bytes_total";
pub const MACHINE_DISK_WRITTEN_BYTES: &str = "pistack_machine_disk_written_bytes_total";
pub const MACHINE_DISK_READS: &str = "pistack_machine_disk_reads_total";
pub const MACHINE_DISK_WRITES: &str = "pistack_machine_disk_writes_total";
pub const MACHINE_NET_RX_BYTES: &str = "pistack_machine_network_receive_bytes_total";
pub const MACHINE_NET_TX_BYTES: &str = "pistack_machine_network_transmit_bytes_total";

pub const BUILD_INFO: &str = "pistack_build_info";
pub const CLUSTER_LEADER: &str = "pistack_cluster_is_leader";
pub const CLUSTER_VOTER: &str = "pistack_cluster_is_voter";
pub const CLUSTER_TERM: &str = "pistack_cluster_term";
pub const CLUSTER_COMMIT: &str = "pistack_cluster_commit_index";
pub const CLUSTER_MEMBERS: &str = "pistack_cluster_members";
pub const CLUSTER_FOLLOWER_LAG: &str = "pistack_cluster_follower_lag_entries";
pub const CLUSTER_FOLLOWER_PULL_AGE: &str = "pistack_cluster_follower_last_pull_seconds";
pub const RECONCILE_FAILURES: &str = "pistack_reconcile_failures_total";
pub const AGENT_QUEUED: &str = "pistack_agent_commands_queued";
pub const AGENT_IN_FLIGHT: &str = "pistack_agent_commands_in_flight";
pub const STORE_FILE_BYTES: &str = "pistack_store_file_bytes";
pub const STORE_LOG_ENTRIES: &str = "pistack_store_log_entries";
pub const STORE_TABLE_ROWS: &str = "pistack_store_table_rows";
pub const STORE_TABLE_BYTES: &str = "pistack_store_table_bytes";
pub const SERIES: &str = "pistack_gauge_series";

const fn gauge(name: &'static str, help: &'static str) -> MetricInfo {
    MetricInfo {
        name,
        kind: MetricKind::Gauge,
        help,
    }
}

const fn counter(name: &'static str, help: &'static str) -> MetricInfo {
    MetricInfo {
        name,
        kind: MetricKind::Counter,
        help,
    }
}

pub static CATALOG: &[MetricInfo] = &[
    gauge(HOST_VCPUS, "CPUs of the host, as its agent reported them."),
    gauge(
        HOST_MEMORY_MIB,
        "Memory of the host in MiB, as its agent reported it.",
    ),
    gauge(HOST_LOAD1, "Load average of the host over one minute."),
    gauge(HOST_LOAD5, "Load average of the host over five minutes."),
    gauge(
        HOST_LOAD15,
        "Load average of the host over fifteen minutes.",
    ),
    gauge(
        HOST_MEMORY_USED,
        "Memory in use on the host: total less what the kernel calls available.",
    ),
    gauge(HOST_MEMORY_TOTAL, "Memory of the host in bytes."),
    gauge(
        HOST_DISK_FREE,
        "Free bytes on the filesystem that holds the agent's data directory.",
    ),
    gauge(
        HOST_DISK_TOTAL,
        "Size in bytes of the filesystem that holds the agent's data directory.",
    ),
    gauge(
        HOST_MACHINES,
        "Machines the host's runtimes hold, running or not.",
    ),
    counter(
        MACHINE_CPU_SECONDS,
        "CPU time the machine has used, in seconds.",
    ),
    gauge(
        MACHINE_CPU_CORES,
        "CPUs the machine kept busy between its host's last two reports.",
    ),
    gauge(
        MACHINE_MEMORY,
        "Memory of the host the machine holds (resident set), in bytes.",
    ),
    gauge(
        MACHINE_MEMORY_ASSIGNED,
        "Memory the machine is given now (the balloon), in bytes.",
    ),
    gauge(
        MACHINE_MEMORY_GUEST_USED,
        "Memory in use inside the guest, when the guest reports it.",
    ),
    counter(
        MACHINE_DISK_READ_BYTES,
        "Bytes the machine has read from a disk.",
    ),
    counter(
        MACHINE_DISK_WRITTEN_BYTES,
        "Bytes the machine has written to a disk.",
    ),
    counter(
        MACHINE_DISK_READS,
        "Read requests the machine has made to a disk.",
    ),
    counter(
        MACHINE_DISK_WRITES,
        "Write requests the machine has made to a disk.",
    ),
    counter(
        MACHINE_NET_RX_BYTES,
        "Bytes the machine has received on an interface.",
    ),
    counter(
        MACHINE_NET_TX_BYTES,
        "Bytes the machine has sent on an interface.",
    ),
    gauge(
        BUILD_INFO,
        "Always 1; the build label is the server's build number.",
    ),
    gauge(
        CLUSTER_LEADER,
        "1 on the server that leads, 0 on the others.",
    ),
    gauge(CLUSTER_VOTER, "1 on a voter, 0 on a learner."),
    gauge(CLUSTER_TERM, "Election term this server is in."),
    gauge(
        CLUSTER_COMMIT,
        "Index of the last entry in this server's log.",
    ),
    gauge(
        CLUSTER_MEMBERS,
        "Servers in the cluster by role, as this server's store has them.",
    ),
    gauge(
        CLUSTER_FOLLOWER_LAG,
        "On the leader: entries a member was behind when it last pulled.",
    ),
    gauge(
        CLUSTER_FOLLOWER_PULL_AGE,
        "On the leader: seconds since a member last pulled the log.",
    ),
    counter(
        RECONCILE_FAILURES,
        "Reconcile passes and steps of a module that ended in an error since this server started.",
    ),
    gauge(
        AGENT_QUEUED,
        "Commands waiting for a host's agent to fetch them.",
    ),
    gauge(
        AGENT_IN_FLIGHT,
        "Commands a host's agent has fetched and not answered.",
    ),
    gauge(STORE_FILE_BYTES, "Size of this server's store file."),
    gauge(
        STORE_LOG_ENTRIES,
        "Replication log entries this server's store holds.",
    ),
    gauge(
        STORE_TABLE_ROWS,
        "Rows in a table of the store, counted every five minutes.",
    ),
    gauge(
        STORE_TABLE_BYTES,
        "Key and value bytes in a table of the store, counted every five minutes.",
    ),
    gauge(SERIES, "Series this server holds in memory."),
];

/// What PiStack knows about `name`, if it is one of its own.
pub fn info(name: &str) -> Option<&'static MetricInfo> {
    CATALOG.iter().find(|m| m.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_reserved_legal_and_listed_once() {
        let mut seen = std::collections::BTreeSet::new();
        for m in CATALOG {
            assert!(m.name.starts_with(RESERVED_PREFIX), "{}", m.name);
            assert_eq!(crate::series::sanitize_name(m.name), m.name);
            assert!(seen.insert(m.name), "{} is listed twice", m.name);
            // Counters end in `_total`, and nothing else does.
            assert_eq!(
                m.kind == MetricKind::Counter,
                m.name.ends_with("_total"),
                "{}",
                m.name
            );
            assert!(!m.help.is_empty() && !m.help.contains('\n'));
        }
    }
}
