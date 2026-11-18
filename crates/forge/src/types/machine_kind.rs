// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash, Default)]
#[serde(rename_all = "snake_case")]
pub enum MachineKind {
    #[default]
    Vm,
    Container,
    Metal,
}

impl MachineKind {
    /// Host driver capabilities that can run this kind. A host advertising
    /// any one of them qualifies. "mock" runs everything (dev mode).
    pub fn accepted_drivers(self) -> &'static [&'static str] {
        match self {
            MachineKind::Vm => &["kvm", "libvirt", "cloud-hypervisor", "mock"],
            MachineKind::Container => &["youki", "mock"],
            MachineKind::Metal => &["metal", "ipmi", "redfish", "mock"],
        }
    }
}
