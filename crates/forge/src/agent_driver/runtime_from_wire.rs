// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::power_from_wire::power_from_wire;
use crate::driver::MachineRuntime;
use pistack_rpc::proto::RunningMachine;
pub fn runtime_from_wire(rt: RunningMachine) -> MachineRuntime {
    MachineRuntime {
        id: rt.id,
        addresses: rt.addresses,
        power: power_from_wire(rt.power),
        message: rt.message,
    }
}
