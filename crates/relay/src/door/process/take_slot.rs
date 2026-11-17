// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Removing a listener's slot from the table, returning it.

use super::doors::doors;
use super::slot::Slot;

pub(crate) fn take_slot(id: &str) -> Option<Slot> {
    doors().lock().remove(id)
}
