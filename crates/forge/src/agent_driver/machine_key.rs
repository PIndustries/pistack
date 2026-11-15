// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::Id;

/// The queue key of the commands that start, stop and delete one machine.
/// One key for the three: a host has one of them outstanding per machine,
/// and a different one replaces it.
pub fn machine_key(id: &Id) -> String {
    format!("machine/{id}")
}
