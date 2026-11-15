// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::probe::book::probe_book::ProbeBook;
use pistack_types::Id;

impl ProbeBook {
    pub fn forget(&self, id: &Id) {
        self.lock().machines.remove(id);
    }
}
