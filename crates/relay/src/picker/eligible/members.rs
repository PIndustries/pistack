// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Members that may take traffic.

use crate::types::{Member, OperatingStatus};

/// Members that may take traffic: administratively up, weighted above
/// zero, and `Online` or unmonitored. Under every algorithm a member
/// outside that is never a choice, so a data plane that dials addresses
/// rather than members starts from this list too.
pub fn eligible(members: &[Member]) -> Vec<&Member> {
    members
        .iter()
        .filter(|m| {
            m.spec.admin_up
                && m.spec.weight > 0
                && matches!(
                    m.status.operating_status,
                    OperatingStatus::Online | OperatingStatus::NoMonitor
                )
        })
        .collect()
}
