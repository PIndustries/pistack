// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::PowerState;
use pistack_types::Id;

/// What a driver knows about a machine it runs.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MachineRuntime {
    pub id: Id,
    pub addresses: Vec<String>,
    pub power: PowerState,
    /// Operator note from the host. Empty when the host has nothing to add.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub message: String,
}
