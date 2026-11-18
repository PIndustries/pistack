// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use serde::{Deserialize, Serialize};

// ---- kinds and well-known keys ------------------------------------------------

use super::default_failure::default_failure;
use super::default_period::default_period;
use super::default_success::default_success;
use super::default_timeout::default_timeout;
use super::exec_probe::ExecProbe;
use super::http_get_probe::HttpGetProbe;
use super::nonzero::nonzero;
use super::probe_handler_kind::ProbeHandlerKind;
use super::tcp_socket_probe::TcpSocketProbe;

/// One check against a container. Exactly one of `http_get`, `tcp_socket`,
/// or `exec` is set. Zero `period_seconds`, `timeout_seconds`,
/// `success_threshold`, or `failure_threshold` means the default.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Probe {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_get: Option<HttpGetProbe>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tcp_socket: Option<TcpSocketProbe>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exec: Option<ExecProbe>,
    #[serde(default)]
    pub initial_delay_seconds: u32,
    #[serde(default = "default_period")]
    pub period_seconds: u32,
    #[serde(default = "default_timeout")]
    pub timeout_seconds: u32,
    #[serde(default = "default_success")]
    pub success_threshold: u32,
    #[serde(default = "default_failure")]
    pub failure_threshold: u32,
}

impl Default for Probe {
    fn default() -> Self {
        Self {
            http_get: None,
            tcp_socket: None,
            exec: None,
            initial_delay_seconds: 0,
            period_seconds: default_period(),
            timeout_seconds: default_timeout(),
            success_threshold: default_success(),
            failure_threshold: default_failure(),
        }
    }
}

impl Probe {
    pub fn period(&self) -> u32 {
        nonzero(self.period_seconds, default_period())
    }
    pub fn timeout(&self) -> u32 {
        nonzero(self.timeout_seconds, default_timeout())
    }
    pub fn successes_required(&self) -> u32 {
        nonzero(self.success_threshold, default_success())
    }
    pub fn failures_required(&self) -> u32 {
        nonzero(self.failure_threshold, default_failure())
    }
    pub fn handler(&self) -> ProbeHandlerKind {
        if self.http_get.is_some() {
            ProbeHandlerKind::Http
        } else if self.tcp_socket.is_some() {
            ProbeHandlerKind::Tcp
        } else if self.exec.is_some() {
            ProbeHandlerKind::Exec
        } else {
            ProbeHandlerKind::Missing
        }
    }
}
