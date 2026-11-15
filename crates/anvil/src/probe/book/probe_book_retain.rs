// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::probe::book::probe_book::ProbeBook;
use pistack_types::Id;
use std::collections::BTreeSet;

impl ProbeBook {
    /// Drop machines that are no longer on the host.
    pub fn retain(&self, live: &BTreeSet<Id>) {
        self.lock().machines.retain(|id, _| live.contains(id));
    }
}
