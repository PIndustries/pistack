// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::Resources;
use serde::{Deserialize, Serialize};

use super::power_control::PowerControl;

/// What a host *is*: reported by the agent at hello time and on change.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct HostSpec {
    /// The agent's node name (usually the hostname).
    pub node_name: String,
    /// Total resources the host offers to the scheduler.
    pub capacity: Resources,
    /// Capabilities the agent detected: "kvm", "libvirt", "youki", "ovs",
    /// "metal", "mock". The scheduler maps `MachineKind` onto these.
    #[serde(default)]
    pub drivers: Vec<String>,
    /// Underlay IPv4 reported by the agent. None means this host is not on
    /// any overlay segment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub underlay_ip: Option<String>,
    /// Set by an operator, not by the agent. The scheduler places nothing
    /// new on a disabled host. Machines already there keep running.
    #[serde(default)]
    pub disabled: bool,
    /// Why the host is disabled, in the operator's words.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub disabled_reason: String,
    /// Out-of-band power control, set by a cloud admin. When present, an
    /// evacuation powers the host off and confirms it before anything moves.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub power_control: Option<PowerControl>,
}
