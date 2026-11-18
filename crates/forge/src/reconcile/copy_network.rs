// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::MachineStatus;

pub(crate) fn copy_network(dest: &mut MachineStatus, src: &MachineStatus) {
    if let Some(net) = src.conditions.iter().find(|c| c.kind == "Network") {
        dest.set_condition("Network", net.status, &net.reason, &net.message);
    } else {
        dest.conditions.retain(|c| c.kind != "Network");
    }
}
