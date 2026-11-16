// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::network::ObservedNet;
use crate::runtime::PowerState;
use crate::runtime::RunningMachine;
use pistack_types::Id;

pub(in crate::runtime::youki) fn running_from(
    id: Id,
    observed: &ObservedNet,
    power: PowerState,
) -> RunningMachine {
    RunningMachine {
        id,
        addresses: observed.addresses.clone(),
        power,
        message: observed.message.clone(),
    }
}
