// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::MachineKind;

pub(crate) fn kind_word(kind: MachineKind) -> &'static str {
    match kind {
        MachineKind::Vm => "virtual machine",
        MachineKind::Container => "container",
        MachineKind::Metal => "bare metal machine",
    }
}
