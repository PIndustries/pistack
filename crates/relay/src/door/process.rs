// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The publisher this process started for a listener, and stopping it.

use std::collections::HashMap;
use std::sync::OnceLock;

pub(super) struct Slot {
    pub(super) argv: Vec<String>,
    pub(super) child: Option<tokio::process::Child>,
    pub(super) one_shot: bool,
}

pub(super) fn doors() -> &'static parking_lot::Mutex<HashMap<String, Slot>> {
    static DOORS: OnceLock<parking_lot::Mutex<HashMap<String, Slot>>> = OnceLock::new();
    DOORS.get_or_init(|| parking_lot::Mutex::new(HashMap::new()))
}

pub(super) fn take_slot(id: &str) -> Option<Slot> {
    doors().lock().remove(id)
}

/// Stop the publisher this process started for `id`. Edge has no process.
/// WireGuard is not deleted here, and `wg-fleet` is never named.
pub fn stop(id: &str) {
    super::edge::edge_published().lock().remove(id);
    stop_process(id);
}

pub(super) fn stop_process(id: &str) {
    if let Some(mut slot) = take_slot(id) {
        if let Some(child) = slot.child.as_mut() {
            let _ = child.start_kill();
        }
    }
}
