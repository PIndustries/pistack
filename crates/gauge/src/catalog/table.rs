// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::builders::{counter, gauge};
use super::metric_info::MetricInfo;
use super::names::*;

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
