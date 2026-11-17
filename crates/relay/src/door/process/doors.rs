// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The process-wide table of publisher slots, keyed by listener id.

use super::slot::Slot;
use std::collections::HashMap;
use std::sync::OnceLock;

pub(crate) fn doors() -> &'static parking_lot::Mutex<HashMap<String, Slot>> {
    static DOORS: OnceLock<parking_lot::Mutex<HashMap<String, Slot>>> = OnceLock::new();
    DOORS.get_or_init(|| parking_lot::Mutex::new(HashMap::new()))
}
