// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::{kind, Id, Meta, Resource};

use super::machine::Machine;
use super::machine_spec::MachineSpec;
use super::machine_status::MachineStatus;

pub fn new_machine(name: impl Into<String>, project: Option<Id>, spec: MachineSpec) -> Machine {
    Machine(Resource {
        meta: Meta::new(kind::MACHINE, name, project),
        spec,
        status: MachineStatus::default(),
    })
}
