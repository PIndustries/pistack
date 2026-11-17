// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! A command that would publish the listener.

/// A command that would publish the listener. Empty `bin` means do not run
/// anything. `note` says why, when there is no command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DoorPlan {
    pub bin: String,
    pub argv: Vec<String>,
    pub note: String,
}
