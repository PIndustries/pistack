// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use chrono::{DateTime, Utc};
use pistack_types::{Condition, Phase, Resources};
use serde::{Deserialize, Serialize};

/// What is true about a host right now. Written by the reconciler.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct HostStatus {
    /// Capacity minus everything forge has placed on the host.
    pub free: Resources,
    /// Last heartbeat / report. A host unseen for `HOST_STALE_AFTER` goes
    /// to `Phase::Error`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_seen: Option<DateTime<Utc>>,
    #[serde(default)]
    pub phase: Phase,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub message: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conditions: Vec<Condition>,
    /// OVN on this host, from the agent's last heartbeat. None when the
    /// agent sent no report. The scheduler places a machine on an OVN
    /// network only where this says ready.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ovn: Option<pistack_rpc::OvnReport>,
    /// When the agent's client certificate expires. Not stored: the host
    /// routes fill it in from keep's certificate records on every read.
    /// None for a host that never enrolled (the dev-mode local host).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cert_expires: Option<DateTime<Utc>>,
}

impl HostStatus {
    /// Keep the agent's OVN report and mirror it as the `OvnReady`
    /// condition. No report removes both.
    pub fn set_ovn(&mut self, report: Option<&pistack_rpc::OvnReport>) {
        if self.ovn.as_ref() == report {
            return;
        }
        self.ovn = report.cloned();
        let Some(report) = report else {
            self.conditions.retain(|c| c.kind != "OvnReady");
            return;
        };
        let reason = if report.ready {
            "ChassisConnected"
        } else {
            "ChassisNotReady"
        };
        match self.conditions.iter_mut().find(|c| c.kind == "OvnReady") {
            Some(c) => {
                if c.status != report.ready {
                    c.last_transition = Utc::now();
                }
                c.status = report.ready;
                c.reason = reason.to_owned();
                c.message = report.message.clone();
            }
            None => self.conditions.push(Condition {
                kind: "OvnReady".to_owned(),
                status: report.ready,
                reason: reason.to_owned(),
                message: report.message.clone(),
                last_transition: Utc::now(),
            }),
        }
    }
}
