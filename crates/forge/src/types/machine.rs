// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::{kind, Kind, Meta, Resource};
use serde::{Deserialize, Serialize};
use std::ops::{Deref, DerefMut};

use super::machine_spec::MachineSpec;
use super::machine_status::MachineStatus;

resource_kind!(
    /// A VM, container or bare-metal machine.
    Machine,
    MachineSpec,
    MachineStatus,
    kind::MACHINE
);
