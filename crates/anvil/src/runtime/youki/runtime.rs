// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::podnet;
/// Host-side OCI runtime. Stateless apart from the bundles on disk; a process
/// wide lock keeps start idempotent when two calls race.
use crate::runtime::PowerState;
use serde::Deserialize;
use serde::Serialize;

#[derive(Debug, Default)]
pub struct YoukiRuntime;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(in crate::runtime::youki) struct Saved {
    pub(super) id: String,
    #[serde(default)]
    pub(super) addresses: Vec<String>,
    #[serde(default)]
    pub(super) power: PowerState,
    /// youki ids of the pod's containers, in spec order. Empty for a
    /// machine that is not a pod sandbox.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) containers: Vec<String>,
    /// Container names in the same order as `containers`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) container_names: Vec<String>,
    /// Probe definitions only. The sandbox document itself is not stored.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) probes: Vec<crate::probe::ContainerProbes>,
    /// Named network namespace created for this pod. Empty when the sandbox
    /// namespace was left alone.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(super) netns: String,
    /// Host ends of the veth pairs. Empty when `netns` is empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) veths: Vec<String>,
    /// OVS ports removed before those veths. Empty on a Linux bridge.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) ovs: Vec<podnet::OvsLink>,
    /// Set when a rootless agent could not attach the recorded address.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(super) network_message: String,
}

pub(super) fn lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}
