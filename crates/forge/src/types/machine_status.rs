// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use chrono::Utc;
use pistack_types::{Condition, Id, Phase};
use serde::{Deserialize, Serialize};

use super::power_state::PowerState;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct MachineStatus {
    #[serde(default)]
    pub phase: Phase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<Id>,
    #[serde(default)]
    pub addresses: Vec<String>,
    #[serde(default)]
    pub conditions: Vec<Condition>,
    #[serde(default)]
    pub power: PowerState,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub message: String,
    /// Restarts the reconciler has performed; compare with `spec.restarts`.
    #[serde(default)]
    pub restarts: u64,
    /// Probe streaks last reported by the host agent. Empty when the
    /// machine has no liveness or readiness probes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub probes: Vec<pistack_rpc::proto::ProbeReport>,
}

impl MachineStatus {
    /// Set a condition, replacing one of the same type; bumps
    /// `last_transition` only when `status` or `reason` changed.
    pub fn set_condition(&mut self, kind: &str, status: bool, reason: &str, message: &str) {
        if let Some(c) = self.conditions.iter_mut().find(|c| c.kind == kind) {
            if c.status != status || c.reason != reason {
                c.last_transition = Utc::now();
            }
            c.status = status;
            c.reason = reason.to_owned();
            c.message = message.to_owned();
            return;
        }
        self.conditions.push(Condition {
            kind: kind.to_owned(),
            status,
            reason: reason.to_owned(),
            message: message.to_owned(),
            last_transition: Utc::now(),
        });
    }
}
