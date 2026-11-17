// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

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
