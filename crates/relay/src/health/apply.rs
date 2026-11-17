// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Folding one probe result into a member's status under the monitor's
//! rise/fall hysteresis.

use super::Probe;
use crate::types::{HealthMonitor, MemberStatus, MonitorType, OperatingStatus};

/// Apply one probe result to a member's status. Returns true when the
/// operating status flipped.
pub fn apply(status: &mut MemberStatus, hm: &HealthMonitor, result: &Probe) -> bool {
    use OperatingStatus::*;
    let was = status.operating_status;
    match (result, was) {
        (Probe::Up, Online) => status.streak = 0,
        (Probe::Up, _) => {
            status.streak += 1;
            if status.streak >= hm.rise {
                status.operating_status = Online;
                status.streak = 0;
            }
        }
        (Probe::Down(_), Offline) => status.streak = 0,
        (Probe::Down(_), _) => {
            status.streak += 1;
            if status.streak >= hm.fall {
                status.operating_status = Offline;
                status.streak = 0;
            }
        }
    }
    status.message = match result {
        Probe::Up => String::new(),
        Probe::Down(m) => m.clone(),
    };
    if hm.kind == MonitorType::Https && status.message.is_empty() {
        status.message = "https monitor: tcp connect only (no tls probe yet)".into();
    }
    status.last_check = Some(chrono::Utc::now());
    status.operating_status != was
}
