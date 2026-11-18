// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::Id;
use std::collections::BTreeMap;

use super::machine_runtime::MachineRuntime;

#[derive(Default)]
pub(crate) struct MockState {
    pub(crate) machines: BTreeMap<Id, MachineRuntime>,
    pub(crate) next_addr: u32,
}
