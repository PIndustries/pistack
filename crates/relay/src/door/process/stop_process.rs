// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Killing the child process in a listener's slot, if it has one.

use super::take_slot::take_slot;

pub(crate) fn stop_process(id: &str) {
    if let Some(mut slot) = take_slot(id) {
        if let Some(child) = slot.child.as_mut() {
            let _ = child.start_kill();
        }
    }
}
