// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::probe::default_failure::default_failure;
use crate::probe::default_period::default_period;
use crate::probe::default_success::default_success;
use crate::probe::default_timeout::default_timeout;
use crate::probe::spec::exec_spec::ExecSpec;
use crate::probe::spec::http_get_spec::HttpGetSpec;
use crate::probe::spec::tcp_socket_spec::TcpSocketSpec;
use serde::{Deserialize, Serialize};

/// Probe fields copied from a container spec. The JSON shape matches the
/// pod spec so a sandbox document deserialises without a translation step.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProbeSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_get: Option<HttpGetSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tcp_socket: Option<TcpSocketSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exec: Option<ExecSpec>,
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
