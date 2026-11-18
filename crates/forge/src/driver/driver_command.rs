// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::Machine;
use pistack_types::Id;

/// One driver call, as a message.
#[derive(Debug)]
pub enum DriverCommand {
    Start(Box<Machine>),
    Stop(Id),
    Delete(Id),
    List,
    Rebuild(Box<Machine>),
}
