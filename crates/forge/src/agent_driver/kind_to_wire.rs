// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::MachineKind;
pub fn kind_to_wire(k: MachineKind) -> pistack_rpc::proto::MachineKind {
    match k {
        MachineKind::Vm => pistack_rpc::proto::MachineKind::Vm,
        MachineKind::Container => pistack_rpc::proto::MachineKind::Container,
        MachineKind::Metal => pistack_rpc::proto::MachineKind::Metal,
    }
}
