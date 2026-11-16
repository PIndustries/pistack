// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::{new_machine, Host, Machine, MachineSpec};
use pistack_types::{Id, Phase};

pub(in crate::overlay) fn placed(name: &str, project: &Id, network: &str, host: &Host) -> Machine {
    let mut machine = new_machine(
        name,
        Some(project.clone()),
        MachineSpec {
            size: "s.1x1".into(),
            image: "ubuntu-24.04".into(),
            networks: vec![network.into()],
            ..MachineSpec::default()
        },
    );
    machine.status.host = Some(host.meta.id.clone());
    machine.status.phase = Phase::Ready;
    machine
}
