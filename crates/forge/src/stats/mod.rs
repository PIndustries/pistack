// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! What hosts and machines are using: a heartbeat's usage report turned
//! into gauge series, and those series read back for one machine or host.
//!
//! The series live in gauge's window on the server that receives the
//! heartbeats, which is the leader. Nothing here is stored.

mod cpu_rate_max_gap_secs;
mod host_samples;
mod host_stats;
mod machine_samples;
mod machine_stats;
mod record_heartbeat;
mod stats_type;
#[cfg(test)]
mod tests;
pub use host_stats::host_stats;
pub use machine_stats::machine_stats;
pub use record_heartbeat::record_heartbeat;
pub use stats_type::Stats;
