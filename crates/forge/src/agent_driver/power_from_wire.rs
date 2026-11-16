// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::PowerState;
pub fn power_from_wire(p: pistack_rpc::proto::PowerState) -> PowerState {
    match p {
        pistack_rpc::proto::PowerState::On => PowerState::On,
        pistack_rpc::proto::PowerState::Off => PowerState::Off,
        pistack_rpc::proto::PowerState::Unknown => PowerState::Unknown,
    }
}
