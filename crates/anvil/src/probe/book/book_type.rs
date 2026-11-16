// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::probe::book::machine_probes::MachineProbes;
use pistack_types::Id;
use std::collections::BTreeMap;

#[derive(Default)]
pub(super) struct Book {
    pub(crate) machines: BTreeMap<Id, MachineProbes>,
}
