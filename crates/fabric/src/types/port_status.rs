// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::Phase;
use serde::{Deserialize, Serialize};

use super::port_binding::PortBinding;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct PortStatus {
    #[serde(default)]
    pub phase: Phase,
    /// Chassis the port is bound on, once the device is placed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binding: Option<PortBinding>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub message: String,
    #[serde(default)]
    pub observed_revision: u64,
}
