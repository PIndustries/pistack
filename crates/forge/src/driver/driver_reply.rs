// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::machine_runtime::MachineRuntime;

/// The answer to a `DriverCommand`.
#[derive(Debug)]
pub enum DriverReply {
    Started(MachineRuntime),
    Done,
    Listed(Vec<MachineRuntime>),
}
