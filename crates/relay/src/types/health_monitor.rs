// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The active health check a pool runs. Absent on a pool means its members
//! are `NoMonitor`.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash, Default)]
#[serde(rename_all = "snake_case")]
pub enum MonitorType {
    #[default]
    Tcp,
    Http,
    Https,
}

/// Active health check settings; absent means members are `NoMonitor`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct HealthMonitor {
    #[serde(default)]
    pub kind: MonitorType,
    /// Seconds between probes.
    #[serde(default = "default_interval")]
    pub interval: u32,
    /// Seconds before a probe is a failure.
    #[serde(default = "default_timeout")]
    pub timeout: u32,
    /// Consecutive successes before Offline -> Online.
    #[serde(default = "default_rise")]
    pub rise: u32,
    /// Consecutive failures before Online -> Offline.
    #[serde(default = "default_fall")]
    pub fall: u32,
    /// HTTP(S) only: request path and accepted status codes.
    #[serde(default = "default_path")]
    pub path: String,
    #[serde(default = "default_codes")]
    pub expected_codes: Vec<u16>,
}

fn default_interval() -> u32 {
    5
}
fn default_timeout() -> u32 {
    3
}
fn default_rise() -> u32 {
    2
}
fn default_fall() -> u32 {
    3
}
fn default_path() -> String {
    "/".into()
}
fn default_codes() -> Vec<u16> {
    vec![200]
}

impl Default for HealthMonitor {
    fn default() -> Self {
        Self {
            kind: MonitorType::Tcp,
            interval: default_interval(),
            timeout: default_timeout(),
            rise: default_rise(),
            fall: default_fall(),
            path: default_path(),
            expected_codes: default_codes(),
        }
    }
}
