// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::MachineStatus;

/// A host that could not attach the recorded address says so here.
/// An empty message drops the condition. The Running condition stays.
pub(crate) fn note_network(status: &mut MachineStatus, message: &str) {
    if message.is_empty() {
        status.conditions.retain(|c| c.kind != "Network");
        return;
    }
    status.message = message.to_owned();
    status.set_condition("Network", false, "Rootless", message);
}
