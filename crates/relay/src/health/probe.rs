// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! One probe and its result, bounded by the monitor's timeout.

use super::probe_inner::probe_inner;
use crate::types::HealthMonitor;
use std::net::SocketAddr;
use std::time::Duration;

/// Result of one probe.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Probe {
    Up,
    Down(String),
}

/// Probe one member with the monitor's settings.
pub async fn probe(addr: SocketAddr, hm: &HealthMonitor) -> Probe {
    let timeout = Duration::from_secs(hm.timeout.max(1) as u64);
    match tokio::time::timeout(timeout, probe_inner(addr, hm)).await {
        Ok(p) => p,
        Err(_) => Probe::Down(format!("timeout after {}s", timeout.as_secs())),
    }
}
